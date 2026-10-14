//! The VIA command handlers, wired to the shared state.
//!
//! This is the seam between RMK's host service and the RGB engine. The host
//! service owns the 32-byte report; this module interprets it and writes the
//! response back into the same buffer.
//!
//! # Why the handlers take `&mut [u8]`
//!
//! `VialService::process_via_packet` seeds `input_data` from `output_data` and
//! lets each arm overwrite only the bytes it changes, so a handler that writes
//! nothing leaves the request echoed back — which is how an unsupported
//! sub-command is signalled. Keeping the same shape here means the call site is
//! a two-liner per VIA command.
//!
//! # Where the state lives
//!
//! Not in the engine. The engine runs in its own task and the host service in
//! another, so the configuration and LED table live in
//! [`super::shared`], which both reach. `0x09` (save) is the one operation that
//! needs the storage task, so it is left to the caller.

use super::fx::color::Hsv;
use super::registry;
use super::shared;
use super::vialrgb::{
    self, Request, DIRECT_LEDS_PER_REPORT, REPORT_SIZE, VIALRGB_GET_INFO, VIALRGB_GET_LED_INFO,
    VIALRGB_GET_MODE, VIALRGB_GET_NUMBER_LEDS, VIALRGB_GET_SUPPORTED, VIALRGB_SET_MODE,
};

/// What a handler did, so the caller can decide about persistence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The report was answered in place.
    Handled,
    /// The sub-command is not implemented; the report is left as it came.
    Unsupported,
    /// The configuration changed and a save should follow.
    Changed,
}

/// `ViaCommand::CustomGetValue` (`0x08`).
///
/// Reads never change anything, so this does not need the caller's cooperation
/// beyond the report buffer.
pub fn handle_get(report: &mut [u8]) -> Outcome {
    if report.len() != REPORT_SIZE {
        return Outcome::Unsupported;
    }
    match vialrgb::decode_get(report) {
        Request::GetInfo => {
            // A keyboard with no LED table cannot serve VialRGB; answer with a
            // zero version so the host reports "unsupported" instead of acting
            // on a brightness of 0.
            let (version_args, max) = shared::with_state(|s| (s.led_count(), s.max_brightness()));
            if version_args == 0 {
                report[1] = 0;
                report[2] = 0;
                report[3] = 0;
                return Outcome::Handled;
            }
            let args = &mut report[2..];
            vialrgb::encode_info(args, max);
            Outcome::Handled
        }
        Request::GetMode => {
            let config = shared::config_snapshot();
            vialrgb::encode_mode(&mut report[2..], &config);
            Outcome::Handled
        }
        Request::GetSupported { gt } => {
            // The enabled list is copied out first: the page encoder fills the
            // report, so holding the lock across it would mix the two.
            let mut enabled = heapless_enabled();
            let count = shared::with_state(|s| {
                let n = s.enabled().len().min(enabled.len());
                enabled[..n].copy_from_slice(&s.enabled()[..n]);
                n
            });
            vialrgb::supported_page(gt, &enabled[..count], &mut report[2..]);
            Outcome::Handled
        }
        Request::GetNumberLeds => {
            let count = shared::with_state(|s| s.led_count());
            let args = &mut report[2..];
            args[0..2].copy_from_slice(&(count as u16).to_le_bytes());
            Outcome::Handled
        }
        Request::GetLedInfo { index } => {
            let Some(info) = shared::led_info(index as usize) else {
                // QMK returns without touching the report for an out-of-range
                // index, leaving whatever the buffer held.
                return Outcome::Handled;
            };
            let args = &mut report[2..];
            args[0] = info.point.x;
            args[1] = info.point.y;
            args[2] = info.flags;
            match info.matrix_pos {
                Some((row, col)) => {
                    args[3] = row;
                    args[4] = col;
                }
                None => {
                    args[3] = super::led_config::NO_LED;
                    args[4] = super::led_config::NO_LED;
                }
            }
            Outcome::Handled
        }
        Request::SetMode { .. } | Request::DirectFastSet { .. } => Outcome::Unsupported,
        Request::Unknown => Outcome::Unsupported,
    }
}

/// `ViaCommand::CustomSetValue` (`0x07`).
pub fn handle_set(report: &mut [u8]) -> Outcome {
    if report.len() != REPORT_SIZE {
        return Outcome::Unsupported;
    }
    // One decode, then one critical section: the mode change and any `0x09`
    // save both describe the same state the engine renders from.
    let request = vialrgb::decode_set(report);
    if let Request::GetInfo
    | Request::GetMode
    | Request::GetSupported { .. }
    | Request::GetNumberLeds
    | Request::GetLedInfo { .. }
    | Request::Unknown = request
    {
        return Outcome::Unsupported;
    }

    let changed = shared::with_state(|s| {
        let was = *s.config();
        let applied = vialrgb::apply_set(s, request);
        // `DIRECT_FASTSET` changes the host's pixels every report, which is not
        // something to persist; only a report that also moved the mode out of
        // effect rendering needs a save.
        if matches!(request, Request::DirectFastSet { .. }) {
            applied && (s.config().mode() != was.mode() || s.config().enable != was.enable)
        } else {
            applied
        }
    });

    if changed {
        // The write path does not touch flash: Vial sends one report per slider
        // step and each must not cost a page write. The flag is what
        // `CustomSave` (0x09) later consumes.
        shared::mark_dirty();
        Outcome::Changed
    } else {
        Outcome::Handled
    }
}

/// A stack buffer for the enabled list, sized like the shared state's.
fn heapless_enabled() -> [u16; shared::MAX_LEDS] {
    [0u16; shared::MAX_LEDS]
}

/// Whether `GET_INFO` would report support.
///
/// The host service uses this to decide whether `CustomGetValue` needs the RGB
/// lock at all.
pub fn is_supported() -> bool {
    shared::is_ready() && shared::with_state(|s| s.led_count()) > 0
}

/// `ViaCommand::CustomSave` (`0x09`): QMK's `eeconfig_force_flush_rgb_matrix`.
///
/// Persisting needs the storage task, which this module cannot reach, so the
/// caller performs the write. This reports whether anything has changed since
/// the last save, without consuming the flag — [`mark_saved`] does that once the
/// write has actually landed.
pub fn needs_save() -> bool {
    shared::is_dirty()
}

/// Clear the dirty flag after a successful write.
pub fn mark_saved() {
    shared::take_dirty();
}

/// The unchanged sub-command ids, re-exported so callers match on one place.
pub const SUB_GET_INFO: u8 = VIALRGB_GET_INFO;
pub const SUB_GET_MODE: u8 = VIALRGB_GET_MODE;
pub const SUB_GET_SUPPORTED: u8 = VIALRGB_GET_SUPPORTED;
pub const SUB_GET_NUMBER_LEDS: u8 = VIALRGB_GET_NUMBER_LEDS;
pub const SUB_GET_LED_INFO: u8 = VIALRGB_GET_LED_INFO;
pub const SUB_SET_MODE: u8 = VIALRGB_SET_MODE;
/// `DIRECT_FASTSET`'s capacity, for the caller's bounds check.
pub const SUB_DIRECT_LEDS_PER_REPORT: usize = DIRECT_LEDS_PER_REPORT;

/// Record a key press for the reactive effects and the heat map.
///
/// This is QMK's `process_rgb_matrix` body, called from the keyboard task's key
/// path rather than from the RGB task: that task owns the engine, so it is the
/// only place that could record a press, and putting the recording here is what
/// lets the engine's run loop stay untouched.
///
/// The caller passes an electrical position; a position with no LED under it is
/// ignored down in [`shared`].
pub fn record_key_event(row: u8, col: u8) {
    shared::with_state(|s| s.record_key_press(row, col));
}

/// Apply an HSV from a keycode handler, clamping to the ceiling.
pub fn set_hsv_from_keycode(hsv: Hsv) -> bool {
    shared::with_state(|s| s.set_hsv(hsv))
}

/// Nudge the configured effect by `forward`, for `RgbModeForward`/`RgbModeReverse`.
pub fn step_mode(forward: bool) -> bool {
    shared::with_state(|s| {
        let config = s.config_mut();
        if forward {
            config.step_mode(registry::EFFECT_MAX)
        } else {
            config.step_mode_reverse(registry::EFFECT_MAX)
        }
    })
}

/// Flip the enabled state, for `RgbTog`.
pub fn toggle() -> bool {
    shared::with_state(|s| {
        s.config_mut().toggle();
        true
    })
}

/// Select an effect by id, for the `RgbMode*` keycodes.
///
/// Returns whether the selection changed. The id is one of the `rgb_matrix`
/// effects the `rgblight` name was mapped onto, so it is a real effect rather
/// than a sentinel.
pub fn select_effect(id: u16) -> bool {
    shared::with_state(|s| {
        if !s.config().is_enabled() {
            return false;
        }
        s.config_mut().set_mode(id, registry::EFFECT_MAX)
    })
}

/// Adjust one component, for the `RgbHui`/`Hud`/`Sai`/`Sad`/`Vai`/`Vad` keycodes.
pub fn adjust(component: Component, increase: bool) -> bool {
    shared::with_state(|s| {
        let max = s.max_brightness();
        let config = s.config_mut();
        match (component, increase) {
            (Component::Hue, true) => config.increase_hue(max),
            (Component::Hue, false) => config.decrease_hue(max),
            (Component::Sat, true) => config.increase_sat(max),
            (Component::Sat, false) => config.decrease_sat(max),
            (Component::Val, true) => config.increase_val(max),
            (Component::Val, false) => config.decrease_val(max),
            (Component::Speed, true) => config.increase_speed(),
            (Component::Speed, false) => config.decrease_speed(),
        }
    })
}

/// Which global the `RgbHui`-family keycodes adjust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Hue,
    Sat,
    Val,
    Speed,
}

/// A snapshot of everything `GET_MODE` and the UI need at once.
pub fn snapshot() -> Option<(super::state::RgbConfig, u8, usize)> {
    shared::with_state(|s| {
        if s.led_count() == 0 {
            None
        } else {
            Some((*s.config(), s.max_brightness(), s.led_count()))
        }
    })
}

/// Seed the shared state from a board definition.
///
/// Called once by the generated board code, before the engine task starts.
pub fn install<const LED_COUNT: usize>(
    table: &super::led_config::LedConfig<LED_COUNT>,
    max_brightness: u8,
    enabled: &[u16],
) {
    shared::with_state(|s| {
        s.set_leds(table);
        s.set_max_brightness(max_brightness);
        s.set_enabled(enabled);
    });
    shared::mark_ready();
}

/// Reset the shared state's configuration, for the storage layer's boot path.
pub fn restore_config(config: super::state::RgbConfig) {
    shared::with_state(|s| *s.config_mut() = config);
}

/// The configuration to persist.
pub fn config_for_storage() -> super::state::RgbConfig {
    shared::config_snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::led_config::{LedConfig, LedPoint, LED_FLAG_KEYLIGHT};
    use crate::rgb::state::RgbConfig;

    const N: usize = 4;

    fn table() -> LedConfig<N> {
        LedConfig::from_matrix(
            [
                LedPoint::new(0, 0),
                LedPoint::new(75, 0),
                LedPoint::new(150, 64),
                LedPoint::new(224, 64),
            ],
            [(0, 0), (0, 1), (1, 0), (1, 1)],
        )
    }

    /// A report with a VIA command and sub-command, nothing else set.
    fn report(command: u8, sub: u8) -> [u8; REPORT_SIZE] {
        let mut data = [0u8; REPORT_SIZE];
        data[0] = command;
        data[1] = sub;
        data
    }

    /// The shared global is process-wide, so these tests install a known table
    /// and restore a known configuration rather than assuming a clean slate.
    fn setup() {
        install(&table(), 200, &[]);
        restore_config(RgbConfig::new_const());
        shared::with_state(|s| {
            s.config_mut().enable();
            s.config_mut().set_hsv(0, 255, 100, 255);
        });
    }

    #[test]
    fn get_info_reports_the_version_and_ceiling() {
        setup();
        let mut data = report(0x08, SUB_GET_INFO);
        assert_eq!(handle_get(&mut data), Outcome::Handled);
        assert_eq!(data[2], 1, "protocol version");
        assert_eq!(data[3], 0);
        assert_eq!(data[4], 200, "the board's ceiling");
    }

    #[test]
    fn get_info_reports_unsupported_without_an_led_table() {
        // An empty table means the board has no RGB wiring.
        install(&LedConfig::<0>::new([], [], []), 255, &[]);
        let mut data = report(0x08, SUB_GET_INFO);
        assert_eq!(handle_get(&mut data), Outcome::Handled);
        assert_eq!(data[2], 0, "version 0 => the host reports unsupported");
        // Restore for the other tests.
        setup();
    }

    #[test]
    fn get_mode_reflects_what_set_mode_wrote() {
        setup();
        let mut set = report(0x07, SUB_SET_MODE);
        set[2..4].copy_from_slice(&24u16.to_le_bytes());
        set[4] = 90;
        set[5] = 10;
        set[6] = 20;
        set[7] = 30;
        assert_eq!(handle_set(&mut set), Outcome::Changed);

        let mut get = report(0x08, SUB_GET_MODE);
        assert_eq!(handle_get(&mut get), Outcome::Handled);
        assert_eq!(u16::from_le_bytes([get[2], get[3]]), 24);
        assert_eq!(get[4], 90, "speed");
        assert_eq!((get[5], get[6], get[7]), (10, 20, 30), "hsv");
    }

    #[test]
    fn set_mode_reports_no_change_when_nothing_moved() {
        setup();
        let mut set = report(0x07, SUB_SET_MODE);
        set[2..4].copy_from_slice(&24u16.to_le_bytes());

        assert_eq!(handle_set(&mut set), Outcome::Changed, "first time selects");
        // The same report again changes nothing, which is what lets the caller
        // skip both a re-render and a flash write.
        assert_eq!(handle_set(&mut set), Outcome::Handled);
    }

    #[test]
    fn set_mode_zero_disables() {
        setup();
        let mut set = report(0x07, SUB_SET_MODE);
        set[2] = 0;
        set[3] = 0;
        assert_eq!(handle_set(&mut set), Outcome::Changed);
        assert!(!shared::config_snapshot().is_enabled());
        setup();
    }

    #[test]
    fn set_mode_direct_is_not_clamped_onto_an_effect() {
        setup();
        let mut set = report(0x07, SUB_SET_MODE);
        set[2..4].copy_from_slice(&vialrgb::DIRECT_MODE.to_le_bytes());
        handle_set(&mut set);
        assert_eq!(shared::config_snapshot().mode(), vialrgb::DIRECT_MODE);
        setup();
    }

    #[test]
    fn brightness_is_clamped_to_the_ceiling() {
        setup();
        let mut set = report(0x07, SUB_SET_MODE);
        set[2..4].copy_from_slice(&24u16.to_le_bytes());
        set[7] = 255; // above the ceiling of 200
        handle_set(&mut set);
        assert_eq!(shared::config_snapshot().val(), 200);
        setup();
    }

    #[test]
    fn get_supported_pages_and_skips_extension_ids() {
        setup();
        shared::with_state(|s| s.set_enabled(&[2, 6, 51]));

        let mut get = report(0x08, SUB_GET_SUPPORTED);
        get[2] = 0;
        get[3] = 0;
        assert_eq!(handle_get(&mut get), Outcome::Handled);

        // 51 is not an effect in this build, so it must not appear.
        let ids: Vec<u16> = (0..8)
            .map(|i| u16::from_le_bytes([get[2 + i * 2], get[3 + i * 2]]))
            .take_while(|id| *id != 0xFFFF)
            .collect();
        assert_eq!(ids, vec![2, 6]);

        setup();
    }

    #[test]
    fn get_number_leds_reports_the_chain_length() {
        setup();
        let mut get = report(0x08, SUB_GET_NUMBER_LEDS);
        assert_eq!(handle_get(&mut get), Outcome::Handled);
        assert_eq!(u16::from_le_bytes([get[2], get[3]]), N as u16);
    }

    #[test]
    fn get_led_info_reports_position_flags_and_matrix_slot() {
        setup();
        let mut get = report(0x08, SUB_GET_LED_INFO);
        get[2] = 2;
        get[3] = 0;
        assert_eq!(handle_get(&mut get), Outcome::Handled);
        assert_eq!(get[2], 150, "x");
        assert_eq!(get[3], 64, "y");
        assert_eq!(get[4], LED_FLAG_KEYLIGHT, "flags");
        assert_eq!((get[5], get[6]), (1, 0), "electrical position");
    }

    #[test]
    fn get_led_info_leaves_the_report_alone_out_of_range() {
        setup();
        let mut get = report(0x08, SUB_GET_LED_INFO);
        get[2] = 200;
        let before = get;
        assert_eq!(handle_get(&mut get), Outcome::Handled);
        assert_eq!(get, before, "QMK returns without writing");
    }

    /// The sub-command space is shared between the two directions: `0x41` is
    /// `GET_MODE` when read and `SET_MODE` when written, `0x42` is
    /// `GET_SUPPORTED` against `DIRECT_FASTSET`. So the same byte is valid on
    /// both paths and the direction alone decides the meaning.
    #[test]
    fn overlapping_subcommands_decode_by_direction() {
        setup();

        // 0x41 as a read is GET_MODE, as a write is SET_MODE.
        let mut get = report(0x08, 0x41);
        assert_eq!(handle_get(&mut get), Outcome::Handled, "0x41 read is GET_MODE");

        let mut set = report(0x07, 0x41);
        assert_ne!(handle_set(&mut set), Outcome::Unsupported, "0x41 write is SET_MODE");

        // 0x42 as a read is GET_SUPPORTED, as a write is DIRECT_FASTSET.
        let mut get = report(0x08, 0x42);
        assert_eq!(handle_get(&mut get), Outcome::Handled, "0x42 read is GET_SUPPORTED");

        setup();
    }

    /// The read-only sub-commands are rejected on the write path.
    #[test]
    fn read_only_subcommands_are_not_accepted_by_the_write_handler() {
        setup();
        // 0x40, 0x43 and 0x44 exist only on the read side.
        for sub in [SUB_GET_INFO, SUB_GET_NUMBER_LEDS, SUB_GET_LED_INFO] {
            let mut data = report(0x07, sub);
            assert_eq!(handle_set(&mut data), Outcome::Unsupported, "sub {sub:#04x}");
        }
    }

    /// Sub-commands above the allocated range are rejected on both paths.
    #[test]
    fn out_of_range_subcommands_are_unsupported() {
        setup();
        for sub in [0x45u8, 0x50, 0xFF] {
            let mut data = report(0x08, sub);
            assert_eq!(handle_get(&mut data), Outcome::Unsupported, "read sub {sub:#04x}");
            let mut data = report(0x07, sub);
            assert_eq!(handle_set(&mut data), Outcome::Unsupported, "write sub {sub:#04x}");
        }
    }

    #[test]
    fn an_unknown_subcommand_is_unsupported() {
        setup();
        let mut data = report(0x08, 0x99);
        assert_eq!(handle_get(&mut data), Outcome::Unsupported);
        let mut data = report(0x07, 0x99);
        assert_eq!(handle_set(&mut data), Outcome::Unsupported);
    }

    #[test]
    fn a_short_report_is_unsupported() {
        setup();
        let mut short = [0u8; 8];
        short[0] = 0x08;
        short[1] = SUB_GET_INFO;
        assert_eq!(handle_get(&mut short), Outcome::Unsupported);
        let mut short = [0u8; 8];
        short[0] = 0x07;
        short[1] = SUB_SET_MODE;
        assert_eq!(handle_set(&mut short), Outcome::Unsupported);
    }

    #[test]
    fn toggle_and_step_and_adjust_act_on_the_shared_config() {
        setup();
        assert!(toggle());
        assert!(!shared::config_snapshot().is_enabled());
        toggle();
        assert!(shared::config_snapshot().is_enabled());

        shared::with_state(|s| {
            s.config_mut().set_mode(6, registry::EFFECT_MAX);
        });
        assert!(step_mode(true));
        assert_eq!(shared::config_snapshot().mode(), 7);
        assert!(step_mode(false));
        assert_eq!(shared::config_snapshot().mode(), 6);

        let before = shared::config_snapshot().hue();
        assert!(adjust(Component::Hue, true));
        assert_ne!(shared::config_snapshot().hue(), before);

        setup();
    }

    #[test]
    fn speed_adjustment_is_not_gated_on_enable() {
        setup();
        shared::with_state(|s| {
            s.config_mut().disable();
            s.config_mut().set_speed(100);
        });
        assert!(adjust(Component::Speed, true));
        assert_eq!(shared::config_snapshot().speed(), 116, "qadd8(100, 16)");
        setup();
    }

    #[test]
    fn snapshot_is_none_without_a_table() {
        install(&LedConfig::<0>::new([], [], []), 255, &[]);
        assert!(snapshot().is_none());
        setup();
    }

    #[test]
    fn is_supported_needs_both_ready_and_a_table() {
        setup();
        assert!(is_supported());
        install(&LedConfig::<0>::new([], [], []), 255, &[]);
        assert!(!is_supported());
        setup();
    }

    /// The whole point of the two halves meeting in [`shared`]: a report Vial
    /// writes must reach the pixels.
    ///
    /// The regression this guards is subtle. The engine keeps its own copy of
    /// the configuration and used to build it once from the compiled-in
    /// defaults, so every `SET_MODE` updated the state Vial reads back while the
    /// chain kept animating whatever it started with — a UI that looked
    /// connected and did nothing.
    ///
    /// The engine is deliberately built at the *same* effect with a different
    /// colour: a test that changed both would pass on a stale frame too, which
    /// is exactly how the original bug slipped through.
    #[test]
    fn a_mode_written_by_vial_reaches_the_engine() {
        use crate::rgb::engine::RgbEngine;
        use crate::rgb::fx::color::{hsv_to_rgb, Hsv};
        use crate::rgb::runner::ArrayFrame;

        setup();
        // The board starts on SOLID_COLOR in red.
        let mut initial = RgbConfig::new_const();
        initial.set_hsv(0, 255, 255, 255);
        restore_config(initial);

        install(&table(), 255, &[]);
        let leds: &'static LedConfig<N> = Box::leak(Box::new(table()));
        let mut engine = RgbEngine::<N, ArrayFrame<N>>::new(leds, initial, 255, ArrayFrame::new());

        run_frame(&mut engine);
        assert_eq!(
            engine.frame().pixels[0],
            hsv_to_rgb(Hsv::new(0, 255, 255)),
            "the board's own colour renders first"
        );

        // Vial asks for the same effect in green.
        let mut set = report(0x07, SUB_SET_MODE);
        set[2..4].copy_from_slice(&registry::EFFECT_FIRST.to_le_bytes());
        set[4] = 0;
        set[5] = 85;
        set[6] = 255;
        set[7] = 255;
        assert_eq!(handle_set(&mut set), Outcome::Changed);

        run_frame(&mut engine);
        let expected = hsv_to_rgb(Hsv::new(85, 255, 255));
        assert_eq!(engine.frame().pixels[0], expected, "Vial's colour renders");
        assert_eq!(engine.frame().pixels[1], expected);

        setup();
    }

    /// Run the machine to completion for one frame.
    ///
    /// The clock is advanced on every step: `Syncing` holds a frame back until
    /// `RGB_MATRIX_LED_FLUSH_LIMIT` has passed, and these tests have no real
    /// timer to advance it for them. Without this the loop exits on the first
    /// `Syncing` and leaves the previous frame in place, which would make a
    /// "the frame changed" assertion pass or fail for the wrong reason.
    fn run_frame<const LED_COUNT: usize, F: crate::rgb::engine::FrameBuffer>(
        engine: &mut crate::rgb::engine::RgbEngine<LED_COUNT, F>,
    ) {
        use crate::rgb::engine::{TaskState, LED_FLUSH_LIMIT_MS};

        fn block_on<Fut: core::future::Future>(fut: Fut) -> Fut::Output {
            use core::task::{Context, Poll, Waker};
            fn noop_waker() -> Waker {
                use core::task::{RawWaker, RawWakerVTable};
                const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
                const RAW: RawWaker = RawWaker::new(core::ptr::null(), &VTABLE);
                // Safety: every vtable entry is a no-op and no state is read
                // through the null data pointer.
                unsafe { Waker::from_raw(RAW) }
            }
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            let mut fut = core::pin::pin!(fut);
            for _ in 0..64 {
                if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                    return out;
                }
            }
            panic!("the RGB driver suspended; these tests use drivers that never await");
        }

        for _ in 0..64 {
            engine.advance_time(engine.timer_ms().wrapping_add(LED_FLUSH_LIMIT_MS));
            if block_on(engine.tick()) == TaskState::Syncing {
                break;
            }
        }
    }
}
