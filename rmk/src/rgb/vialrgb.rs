//! The VialRGB wire protocol: port of vial-qmk's `quantum/vialrgb.c`.
//!
//! Vial and OpenRGB drive the RGB Matrix through three VIA commands, each
//! carrying a 32-byte report whose second byte is a sub-command:
//!
//! | VIA command | id | sub-commands |
//! |---|---|---|
//! | `CustomSetValue` | `0x07` | `0x41` `SET_MODE`, `0x42` `DIRECT_FASTSET` |
//! | `CustomGetValue` | `0x08` | `0x40` `GET_INFO`, `0x41` `GET_MODE`, `0x42` `GET_SUPPORTED`, `0x43` `GET_NUMBER_LEDS`, `0x44` `GET_LED_INFO` |
//! | `CustomSave` | `0x09` | none |
//!
//! Everything here is byte-in/byte-out and free of hardware, so the framing is
//! testable without a keyboard. [`VialRgb`] is what the host service holds; the
//! engine is reached through the callbacks it takes.
//!
//! # Two details worth knowing
//!
//! **`SET_MODE` carries the whole configuration.** Vial sends mode, speed, hue,
//! saturation and value in one report, not incrementally, so a single call can
//! change all five.
//!
//! **`GET_SUPPORTED` is paged.** The host passes the highest id it has seen and
//! the firmware answers with every id above it, filling the rest of the report
//! with `0xFF`. Vial loops until the ids stop growing, so a firmware that
//! repeats an id would hang the GUI; [`supported_page`] is written so the
//! returned ids always strictly increase.

use super::engine::RgbEngine;
use super::led_config::LedConfig;
use super::registry;
use super::state::RgbConfig;

/// `VIALRGB_PROTOCOL_VERSION`, from `quantum/vialrgb.h`.
///
/// Vial rejects any other value outright with "Unsupported VialRGB protocol",
/// so this is a hard contract rather than a capability advertisement.
pub const VIALRGB_PROTOCOL_VERSION: u16 = 1;

/// `VIALRGB_GET_INFO`.
pub const VIALRGB_GET_INFO: u8 = 0x40;
/// `VIALRGB_GET_MODE`, and `VIALRGB_SET_MODE` on the write path.
pub const VIALRGB_GET_MODE: u8 = 0x41;
/// `VIALRGB_GET_SUPPORTED`, and `VIALRGB_DIRECT_FASTSET` on the write path.
pub const VIALRGB_GET_SUPPORTED: u8 = 0x42;
/// `VIALRGB_GET_NUMBER_LEDS`.
pub const VIALRGB_GET_NUMBER_LEDS: u8 = 0x43;
/// `VIALRGB_GET_LED_INFO`.
pub const VIALRGB_GET_LED_INFO: u8 = 0x44;
/// `VIALRGB_SET_MODE` (same value as `GET_MODE`, different direction).
pub const VIALRGB_SET_MODE: u8 = 0x41;
/// `VIALRGB_DIRECT_FASTSET`.
pub const VIALRGB_DIRECT_FASTSET: u8 = 0x42;

/// `VIAL_RAW_EPSIZE`: every report is exactly this long.
pub const REPORT_SIZE: usize = 32;

/// `DIRECT_FASTSET`'s per-report LED capacity: `(32 - 2 - 2 - 1) / 3`.
pub const DIRECT_LEDS_PER_REPORT: usize = 9;

/// `LED_FLAG_*` values that `GET_LED_INFO` reports, from `rgb_matrix_types.h`.
pub use super::led_config::{
    LED_FLAG_INDICATOR, LED_FLAG_KEYLIGHT, LED_FLAG_MODIFIER, LED_FLAG_NONE, LED_FLAG_UNDERGLOW, NO_LED,
};

/// What the host asked for, decoded from a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `GET_INFO`: no arguments.
    GetInfo,
    /// `GET_MODE`: no arguments.
    GetMode,
    /// `GET_SUPPORTED`: report every id above `gt`.
    GetSupported { gt: u16 },
    /// `GET_NUMBER_LEDS`: no arguments.
    GetNumberLeds,
    /// `GET_LED_INFO`: describe LED `index`.
    GetLedInfo { index: u16 },
    /// `SET_MODE`: the whole configuration at once.
    SetMode { mode: u16, speed: u8, hsv: (u8, u8, u8) },
    /// `DIRECT_FASTSET`: per-LED HSV, `count` of them from `first`.
    DirectFastSet { first: u16, hsv: [(u8, u8, u8); DIRECT_LEDS_PER_REPORT], count: u8 },
    /// A sub-command this firmware does not implement, or a truncated report.
    Unknown,
}

/// Decode a `CustomGetValue` (`0x08`) report.
///
/// The report is the full 32 bytes: `data[0]` is the VIA command, `data[1]` the
/// sub-command, and the arguments start at `data[2]`.
pub fn decode_get(data: &[u8]) -> Request {
    if data.len() != REPORT_SIZE {
        return Request::Unknown;
    }
    match data[1] {
        VIALRGB_GET_INFO => Request::GetInfo,
        VIALRGB_GET_MODE => Request::GetMode,
        VIALRGB_GET_SUPPORTED => Request::GetSupported {
            // VialRGB arguments are little-endian.
            gt: u16::from_le_bytes([data[2], data[3]]),
        },
        VIALRGB_GET_NUMBER_LEDS => Request::GetNumberLeds,
        VIALRGB_GET_LED_INFO => Request::GetLedInfo {
            index: u16::from_le_bytes([data[2], data[3]]),
        },
        _ => Request::Unknown,
    }
}

/// Decode a `CustomSetValue` (`0x07`) report.
pub fn decode_set(data: &[u8]) -> Request {
    if data.len() != REPORT_SIZE {
        return Request::Unknown;
    }
    match data[1] {
        VIALRGB_SET_MODE => Request::SetMode {
            mode: u16::from_le_bytes([data[2], data[3]]),
            speed: data[4],
            hsv: (data[5], data[6], data[7]),
        },
        VIALRGB_DIRECT_FASTSET => {
            let first = u16::from_le_bytes([data[2], data[3]]);
            let count = data[4].min(DIRECT_LEDS_PER_REPORT as u8);
            let mut hsv = [(0u8, 0u8, 0u8); DIRECT_LEDS_PER_REPORT];
            for (i, slot) in hsv.iter_mut().enumerate().take(count as usize) {
                let base = 5 + i * 3;
                *slot = (data[base], data[base + 1], data[base + 2]);
            }
            Request::DirectFastSet { first, hsv, count }
        }
        _ => Request::Unknown,
    }
}

/// Write the `GET_INFO` response payload into `args` (the report from offset 2).
pub fn encode_info(args: &mut [u8], max_brightness: u8) {
    args[0..2].copy_from_slice(&VIALRGB_PROTOCOL_VERSION.to_le_bytes());
    args[2] = max_brightness;
}

/// Write the `GET_MODE` response payload.
///
/// The layout is mode (`u16` little-endian), then speed, then hue, saturation and
/// value — the same five bytes `SET_MODE` accepts, so a host can read back
/// exactly what it wrote.
pub fn encode_mode(args: &mut [u8], config: &RgbConfig) {
    args[0..2].copy_from_slice(&config.mode().to_le_bytes());
    args[2] = config.speed();
    args[3] = config.hue();
    args[4] = config.sat();
    args[5] = config.val();
}

/// Write a `GET_SUPPORTED` page and return how many ids were written.
///
/// `args` is the payload buffer (report from offset 2). It is filled with
/// `0xFF` first, exactly as `get_supported()` does, because the host reads
/// pairs until it hits the sentinel.
///
/// Only ids above `gt` and at most [`VIALRGB_LAST`] are reported: a host asked
/// for VialRGB ids, and an extension id would make it select the wrong effect.
pub fn supported_page(gt: u16, enabled: &[u16], args: &mut [u8]) -> usize {
    args.fill(0xFF);

    // At most one id per two bytes.
    let capacity = args.len() / 2;
    let mut scratch = [0u16; 32];
    let count = registry::supported_ids(gt, enabled, &mut scratch[..capacity.min(32)]);

    for (i, id) in scratch[..count].iter().enumerate() {
        args[i * 2..i * 2 + 2].copy_from_slice(&id.to_le_bytes());
    }
    count
}

/// Write the `GET_LED_INFO` response payload.
///
/// `flags` and the matrix position come straight from the LED table. An LED that
/// sits under no key reports `(NO_LED, NO_LED)`, which is QMK's
/// `get_matrix_pos_for_led` sentinel.
///
/// Returns `false` when `index` is out of range; QMK returns without modifying
/// the report in that case, leaving whatever the buffer held.
pub fn encode_led_info<const LED_COUNT: usize>(
    args: &mut [u8],
    leds: &LedConfig<LED_COUNT>,
    index: u16,
) -> bool {
    if index as usize >= LED_COUNT {
        return false;
    }
    let i = index as usize;
    let point = leds.point(i);
    args[0] = point.x;
    args[1] = point.y;
    args[2] = leds.flags(i);
    match leds.matrix_pos(i) {
        Some((row, col)) => {
            args[3] = row;
            args[4] = col;
        }
        None => {
            args[3] = NO_LED;
            args[4] = NO_LED;
        }
    }
    true
}

/// `DIRECT_FASTSET`'s effect id: the host drives each LED itself.
///
/// VialRGB reserves id 1 for this; it is not a QMK effect, so it is handled
/// here rather than in the registry. It sits *below* the first real effect, so
/// it must bypass [`RgbConfig::set_mode`]'s clamping — storing it directly is
/// what makes [`DirectColors::is_active`] the authority on whether the engine
/// should render from the host's buffer.
pub const DIRECT_MODE: u16 = registry::EFFECT_DIRECT;

/// Whether the configuration currently selects host-driven colours.
#[inline]
pub fn is_direct_mode(config: &RgbConfig) -> bool {
    config.mode() == DIRECT_MODE
}

/// Select host-driven colours, bypassing the effect-range clamp.
fn enter_direct_mode(config: &mut RgbConfig) -> bool {
    let changed = !config.is_enabled() || config.mode() != DIRECT_MODE;
    config.enable();
    config.set_mode_raw(DIRECT_MODE);
    changed
}

/// Apply a decoded write request to the engine.
///
/// Returns `true` when the configuration changed, so a caller can decide whether
/// to persist.
pub fn apply_set<const LED_COUNT: usize, F: super::engine::FrameBuffer>(
    engine: &mut RgbEngine<LED_COUNT, F>,
    request: Request,
) -> bool {
    match request {
        Request::SetMode { mode, speed, hsv } => {
            let max = engine.max_brightness();
            let mut changed = false;
            // QMK's `set_mode` first: id 0 disables, anything else enables and
            // selects. `rgb_matrix_enable_noeeprom`/`disable_noeeprom`.
            let config = engine.config_mut();
            if mode == registry::EFFECT_OFF {
                changed |= config.is_enabled();
                config.disable();
            } else if mode == DIRECT_MODE {
                changed |= enter_direct_mode(config);
            } else {
                changed |= !config.is_enabled();
                config.enable();
                changed |= config.set_mode(mode, registry::EFFECT_MAX);
            }
            // Speed is not gated on `enable`, matching QMK.
            changed |= config.set_speed(speed);
            changed |= config.set_hsv(hsv.0, hsv.1, hsv.2, max);

            // Selecting a real effect hands the pixels back to the engine, so a
            // later return to direct mode starts from black rather than the host's
            // stale frame.
            if mode != DIRECT_MODE && engine.direct_colors().is_active() {
                engine.direct_colors_mut().clear();
                changed = true;
            }
            changed
        }
        Request::DirectFastSet { first, hsv, count } => {
            // The host owns these pixels until it sends `SET_MODE` again.
            enter_direct_mode(engine.config_mut());
            let max = engine.max_brightness();
            let colors = engine.direct_colors_mut();
            for (i, (h, s, v)) in hsv.iter().enumerate().take(count as usize) {
                let index = first as usize + i;
                if index >= LED_COUNT {
                    break;
                }
                colors.set(index, *h, *s, *v, max);
            }
            true
        }
        _ => false,
    }
}

/// The per-LED colours `DIRECT_FASTSET` writes, QMK's `g_direct_mode_colors`.
///
/// Stored as HSV because that is what the wire carries, and because the final
/// value must be clamped to the board's brightness ceiling on the way in (as
/// `fast_set_leds` does) rather than on the way out.
#[derive(Clone, Debug)]
pub struct DirectColors<const LED_COUNT: usize> {
    hsv: [(u8, u8, u8); LED_COUNT],
    /// True once the host has written at least one pixel, so the engine knows to
    /// render from this buffer instead of an effect.
    active: bool,
}

impl<const LED_COUNT: usize> Default for DirectColors<LED_COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const LED_COUNT: usize> DirectColors<LED_COUNT> {
    pub const fn new() -> Self {
        Self {
            hsv: [(0, 0, 0); LED_COUNT],
            active: false,
        }
    }

    /// Record one LED's colour, clamping the value as `fast_set_leds` does.
    pub fn set(&mut self, index: usize, h: u8, s: u8, v: u8, max_brightness: u8) {
        self.hsv[index] = (h, s, v.min(max_brightness));
        self.active = true;
    }

    /// Whether the host has written any pixel since construction.
    #[inline]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Forget the host's pixels, returning to effect rendering.
    pub fn clear(&mut self) {
        self.hsv = [(0, 0, 0); LED_COUNT];
        self.active = false;
    }

    /// One LED's stored HSV.
    #[inline]
    pub const fn get(&self, index: usize) -> (u8, u8, u8) {
        self.hsv[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::led_config::{LedConfig, LedPoint};

    const N: usize = 4;

    fn report(command: u8, sub: u8) -> [u8; REPORT_SIZE] {
        let mut data = [0u8; REPORT_SIZE];
        data[0] = command;
        data[1] = sub;
        data
    }

    fn leds() -> LedConfig<N> {
        LedConfig::new(
            [
                LedPoint::new(0, 0),
                LedPoint::new(75, 0),
                LedPoint::new(150, 64),
                LedPoint::new(112, 32),
            ],
            [
                LED_FLAG_KEYLIGHT,
                LED_FLAG_MODIFIER,
                LED_FLAG_UNDERGLOW,
                LED_FLAG_INDICATOR,
            ],
            // The last LED sits under no key.
            [(0, 0), (0, 1), (1, 0), LedConfig::<N>::UNMAPPED],
        )
    }

    #[test]
    fn info_reports_version_1_and_the_brightness_ceiling() {
        let mut args = [0u8; 8];
        encode_info(&mut args, 128);
        assert_eq!(args[0], 1, "protocol version low byte");
        assert_eq!(args[1], 0, "protocol version high byte");
        assert_eq!(args[2], 128, "max brightness");
        // Vial raises RuntimeError unless the version is exactly 1.
        assert_eq!(VIALRGB_PROTOCOL_VERSION, 1);
    }

    #[test]
    fn mode_round_trips_through_encode_and_decode() {
        let mut config = RgbConfig::default();
        config.set_mode(24, registry::EFFECT_MAX);
        config.set_speed(90);
        config.set_hsv(10, 20, 30, 255);

        let mut args = [0u8; 8];
        encode_mode(&mut args, &config);

        // The same five bytes go back in through `SET_MODE`.
        let mut data = report(0x07, VIALRGB_SET_MODE);
        data[2..4].copy_from_slice(&args[0..2]);
        data[4] = args[2];
        data[5] = args[3];
        data[6] = args[4];
        data[7] = args[5];

        match decode_set(&data) {
            Request::SetMode { mode, speed, hsv } => {
                assert_eq!(mode, 24);
                assert_eq!(speed, 90);
                assert_eq!(hsv, (10, 20, 30));
            }
            other => panic!("expected SetMode, got {other:?}"),
        }
    }

    #[test]
    fn get_commands_decode_with_little_endian_arguments() {
        let mut data = report(0x08, VIALRGB_GET_SUPPORTED);
        data[2] = 0x2C; // 44
        data[3] = 0x00;
        assert_eq!(decode_get(&data), Request::GetSupported { gt: 44 });

        let mut data = report(0x08, VIALRGB_GET_LED_INFO);
        data[2] = 0x01;
        data[3] = 0x02; // 0x0201 = 513
        assert_eq!(decode_get(&data), Request::GetLedInfo { index: 513 });
    }

    #[test]
    fn the_other_get_commands_take_no_arguments() {
        for sub in [VIALRGB_GET_INFO, VIALRGB_GET_MODE, VIALRGB_GET_NUMBER_LEDS] {
            let data = report(0x08, sub);
            let expected = match sub {
                VIALRGB_GET_INFO => Request::GetInfo,
                VIALRGB_GET_MODE => Request::GetMode,
                _ => Request::GetNumberLeds,
            };
            assert_eq!(decode_get(&data), expected);
        }
    }

    #[test]
    fn unknown_subcommands_and_wrong_lengths_are_rejected() {
        assert_eq!(decode_get(&report(0x08, 0x99)), Request::Unknown);
        assert_eq!(decode_set(&report(0x07, 0x99)), Request::Unknown);
        // A report of the wrong size is never decoded: every handler starts with
        // `if (length != VIAL_RAW_EPSIZE) return;`.
        assert_eq!(decode_get(&[0x08, VIALRGB_GET_INFO]), Request::Unknown);
        assert_eq!(decode_set(&[0x07, VIALRGB_SET_MODE]), Request::Unknown);
    }

    #[test]
    fn direct_fastset_decodes_up_to_nine_leds() {
        let mut data = report(0x07, VIALRGB_DIRECT_FASTSET);
        data[2] = 1; // first = 1
        data[3] = 0;
        data[4] = 3; // three LEDs
        for i in 0..3 {
            data[5 + i * 3] = 10 + i as u8;
            data[6 + i * 3] = 20 + i as u8;
            data[7 + i * 3] = 30 + i as u8;
        }

        match decode_set(&data) {
            Request::DirectFastSet { first, hsv, count } => {
                assert_eq!(first, 1);
                assert_eq!(count, 3);
                assert_eq!(hsv[0], (10, 20, 30));
                assert_eq!(hsv[2], (12, 22, 32));
            }
            other => panic!("expected DirectFastSet, got {other:?}"),
        }
    }

    #[test]
    fn direct_fastset_clamps_an_oversized_count() {
        let mut data = report(0x07, VIALRGB_DIRECT_FASTSET);
        data[4] = 200; // far more than fits
        match decode_set(&data) {
            Request::DirectFastSet { count, .. } => {
                assert_eq!(count as usize, DIRECT_LEDS_PER_REPORT, "clamped to what fits");
            }
            other => panic!("expected DirectFastSet, got {other:?}"),
        }
    }

    /// The paging contract. Vial loops `while max_effect < 0xFFFF` and always
    /// sends the highest id it has seen, so every page must advance or the GUI
    /// spins forever.
    #[test]
    fn supported_pages_advance_and_terminate() {
        let enabled = [2u16, 6, 13, 24, 31, 32, 51];
        let mut args = [0u8; REPORT_SIZE - 2];
        let mut gt = 0u16;
        let mut seen = Vec::new();
        let mut guard = 0;

        loop {
            let count = supported_page(gt, &enabled, &mut args);
            guard += 1;
            assert!(guard < 16, "paging did not terminate");

            // Read back the little-endian pairs until the 0xFF sentinel.
            let mut page = Vec::new();
            for i in 0..count {
                let id = u16::from_le_bytes([args[i * 2], args[i * 2 + 1]]);
                page.push(id);
            }

            if page.is_empty() {
                break;
            }
            // Strictly increasing, which is what stops the host looping.
            for pair in page.windows(2) {
                assert!(pair[1] > pair[0], "ids must strictly increase: {page:?}");
            }
            let new_gt = *page.last().unwrap();
            assert!(new_gt > gt, "gt must advance: {gt} -> {new_gt}");
            gt = new_gt;
            seen.extend(page);
        }

        // RIVERFLOW (51) is an extension id and must never be reported.
        assert_eq!(seen, vec![2, 6, 13, 24, 31, 32]);
        assert!(!seen.contains(&51));
    }

    #[test]
    fn supported_pages_fill_the_rest_with_the_sentinel() {
        let enabled = [2u16, 6];
        let mut args = [0u8; 8];
        let count = supported_page(0, &enabled, &mut args);
        assert_eq!(count, 2);
        assert_eq!(&args[0..4], &[2, 0, 6, 0]);
        assert_eq!(&args[4..], &[0xFF, 0xFF, 0xFF, 0xFF], "padding is 0xFF");
    }

    #[test]
    fn supported_never_reports_an_extension_id() {
        // A build where only RIVERFLOW (51) is ported reports nothing to Vial.
        let mut args = [0u8; REPORT_SIZE - 2];
        assert_eq!(supported_page(0, &[51], &mut args), 0);
        assert!(args.iter().all(|b| *b == 0xFF));
        // Sanity: the filter bounds are the ones under test.
        assert!(registry::VIALRGB_LAST < 51);
    }

    #[test]
    fn led_info_reports_position_flags_and_matrix_slot() {
        let leds = leds();

        let mut args = [0u8; 8];
        assert!(encode_led_info(&mut args, &leds, 1));
        assert_eq!(args[0], 75, "x");
        assert_eq!(args[1], 0, "y");
        assert_eq!(args[2], LED_FLAG_MODIFIER, "flags");
        assert_eq!((args[3], args[4]), (0, 1), "electrical position");
    }

    #[test]
    fn an_unmapped_led_reports_the_no_led_sentinel() {
        let leds = leds();
        let mut args = [0u8; 8];
        assert!(encode_led_info(&mut args, &leds, 3));
        assert_eq!(
            (args[3], args[4]),
            (NO_LED, NO_LED),
            "an LED under no key reports 0xFF, 0xFF"
        );
        // It still has a position and flags, which is what makes an underglow
        // visible in a host's LED map.
        assert_eq!((args[0], args[1]), (112, 32));
        assert_eq!(args[2], LED_FLAG_INDICATOR);
    }

    #[test]
    fn led_info_out_of_range_leaves_the_report_untouched() {
        let leds = leds();
        let mut args = [0xAAu8; 8];
        assert!(!encode_led_info(&mut args, &leds, N as u16));
        assert!(args.iter().all(|b| *b == 0xAA), "QMK returns without writing");
    }

    #[test]
    fn direct_colors_clamp_brightness_and_track_activation() {
        let mut colors = DirectColors::<N>::new();
        assert!(!colors.is_active());

        colors.set(0, 10, 20, 255, 128);
        assert!(colors.is_active());
        assert_eq!(colors.get(0), (10, 20, 128), "value clamped to the ceiling");

        colors.clear();
        assert!(!colors.is_active());
        assert_eq!(colors.get(0), (0, 0, 0));
    }

    #[test]
    fn set_mode_zero_disables_and_a_real_id_enables() {
        use crate::rgb::engine::RgbEngine;
        use crate::rgb::runner::ArrayFrame;

        let leds = Box::leak(Box::new(leds()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, RgbConfig::default(), 200, ArrayFrame::new());
        let mut direct = DirectColors::<N>::new();

        // Disable via mode 0.
        assert!(apply_set(
            &mut engine,
            Request::SetMode {
                mode: 0,
                speed: 0,
                hsv: (0, 0, 0)
            }
            ));
        assert!(!engine.config().is_enabled());

        // A real id enables and selects, clamping brightness to the ceiling.
        assert!(apply_set(
            &mut engine,
            Request::SetMode {
                mode: 24,
                speed: 100,
                hsv: (10, 20, 255)
            }
            ));
        assert!(engine.config().is_enabled());
        assert_eq!(engine.config().mode(), 24);
        assert_eq!(engine.config().speed(), 100);
        assert_eq!(engine.config().val(), 200, "clamped to max_brightness");
    }

    #[test]
    fn direct_fastset_activates_direct_mode() {
        use crate::rgb::engine::RgbEngine;
        use crate::rgb::runner::ArrayFrame;

        let leds = Box::leak(Box::new(leds()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, RgbConfig::default(), 255, ArrayFrame::new());

        let mut hsv = [(0u8, 0u8, 0u8); DIRECT_LEDS_PER_REPORT];
        hsv[0] = (1, 2, 3);
        hsv[1] = (4, 5, 6);

        assert!(apply_set(
            &mut engine,
            Request::DirectFastSet {
                first: 0,
                hsv,
                count: 2
            }
        ));

        assert!(engine.config().is_enabled());
        assert_eq!(engine.config().mode(), DIRECT_MODE);
        assert!(is_direct_mode(engine.config()));

        let direct = engine.direct_colors();
        assert!(direct.is_active());
        assert_eq!(direct.get(0), (1, 2, 3));
        assert_eq!(direct.get(1), (4, 5, 6));
        assert_eq!(direct.get(2), (0, 0, 0), "untouched LEDs stay black");
    }

    /// The whole point of DIRECT mode: the host's pixels reach the frame.
    #[test]
    fn direct_mode_renders_the_hosts_pixels_into_the_frame() {
        use crate::rgb::engine::{RgbEngine, TaskState};
        use crate::rgb::fx::color::{hsv_to_rgb, Hsv};
        use crate::rgb::runner::ArrayFrame;

        let leds = Box::leak(Box::new(leds()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, RgbConfig::default(), 255, ArrayFrame::new());

        let mut hsv = [(0u8, 0u8, 0u8); DIRECT_LEDS_PER_REPORT];
        hsv[0] = (10, 200, 100);
        hsv[1] = (200, 50, 80);
        apply_set(
            &mut engine,
            Request::DirectFastSet {
                first: 0,
                hsv,
                count: 2,
            },
        );

        // Drive one frame to completion.
        for _ in 0..16 {
            if engine.tick_sync() == TaskState::Syncing {
                break;
            }
        }

        assert_eq!(engine.frame().pixels[0], hsv_to_rgb(Hsv::new(10, 200, 100)));
        assert_eq!(engine.frame().pixels[1], hsv_to_rgb(Hsv::new(200, 50, 80)));
        assert_eq!(engine.frame().pixels[2], crate::rgb::fx::color::Rgb::BLACK);
    }

    /// Leaving direct mode hands the pixels back to the effect engine, so a
    /// later return starts from black rather than a stale host frame.
    #[test]
    fn selecting_an_effect_clears_the_direct_pixels() {
        use crate::rgb::engine::RgbEngine;
        use crate::rgb::runner::ArrayFrame;

        let leds = Box::leak(Box::new(leds()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, RgbConfig::default(), 255, ArrayFrame::new());

        let mut hsv = [(0u8, 0u8, 0u8); DIRECT_LEDS_PER_REPORT];
        hsv[0] = (1, 2, 3);
        apply_set(
            &mut engine,
            Request::DirectFastSet {
                first: 0,
                hsv,
                count: 1,
            },
        );
        assert!(engine.direct_colors().is_active());

        apply_set(
            &mut engine,
            Request::SetMode {
                mode: 24,
                speed: 0,
                hsv: (0, 0, 0),
            },
        );
        assert!(!engine.direct_colors().is_active(), "returned to effect mode");
        assert!(!is_direct_mode(engine.config()));
    }

    #[test]
    fn direct_fastset_ignores_leds_past_the_chain() {
        use crate::rgb::engine::RgbEngine;
        use crate::rgb::runner::ArrayFrame;

        let leds = Box::leak(Box::new(leds()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, RgbConfig::default(), 255, ArrayFrame::new());

        let mut hsv = [(9u8, 9u8, 9u8); DIRECT_LEDS_PER_REPORT];
        hsv[0] = (1, 1, 1);
        // Starting at N - 1 with a count of 9 writes one LED and stops.
        apply_set(
            &mut engine,
            Request::DirectFastSet {
                first: (N - 1) as u16,
                hsv,
                count: 9,
            },
        );
        let direct = engine.direct_colors();
        assert_eq!(direct.get(N - 1), (1, 1, 1));
        // Nothing beyond the chain exists to write.
        assert_eq!(direct.get(0), (0, 0, 0));
    }

    #[test]
    fn a_read_request_changes_nothing() {
        use crate::rgb::engine::RgbEngine;
        use crate::rgb::runner::ArrayFrame;

        let leds = Box::leak(Box::new(leds()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, RgbConfig::default(), 255, ArrayFrame::new());
        let mut direct = DirectColors::<N>::new();
        assert!(!apply_set(&mut engine, Request::GetInfo));
        assert!(!apply_set(&mut engine, Request::Unknown));
    }
}
