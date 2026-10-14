//! The shared RGB state that the Vial protocol and the engine both reach.
//!
//! The host service (`0x07`/`0x08`/`0x09`) runs in its own task, and the engine
//! runs in another. Both need the same configuration, so it lives here rather
//! than inside the engine.
//!
//! # Why a blocking mutex, not a channel
//!
//! RMK's storage uses a request channel with replies, which suits flash: writes
//! are rare, slow and asynchronous. RGB is the opposite — Vial does a handful of
//! small reads and writes while connecting, and the engine reads the config
//! every frame. A channel would add an await and a task hop to a read that must
//! not block, for no benefit.
//!
//! An `embassy_sync::blocking_mutex::Mutex` over a plain value is the right
//! shape: the critical section is a few byte copies with no await in it, which
//! is what `blocking_mutex` requires. The engine copies the config out once per
//! frame and works on the copy, so the lock is never held across a render.
//!
//! # The LED table
//!
//! The table is flattened into a fixed-size array, because a `static` cannot
//! carry the board's const-generic LED count. [`MAX_LEDS`] bounds it; `led_count`
//! records how much of the array is live.

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::RawMutex;

use super::fx::color::Hsv;
use super::led_config::{LedInfo, LED_FLAG_ALL, LED_FLAG_NONE};
use super::state::RgbConfig;

/// The most LEDs a board may have.
///
/// 128 bytes of `LedInfo` each, so this costs 1 KB of `.bss` when the board has
/// fewer — acceptable for a feature that exists to drive a light chain, and it
/// keeps the shared state a plain `static` with no allocator.
pub const MAX_LEDS: usize = 128;

/// The configuration and LED table shared between the protocol and the engine.
#[derive(Clone, Copy, Debug)]
pub struct SharedRgbState {
    config: RgbConfig,
    /// `RGB_MATRIX_MAXIMUM_BRIGHTNESS`.
    max_brightness: u8,
    /// `RGB_MATRIX_LED_PROCESS_LIMIT`.
    process_limit: u8,
    /// How much of `leds` is live.
    led_count: u8,
    /// The board's LED table, flattened.
    leds: [LedInfo; MAX_LEDS],
    /// The effects this build enabled, as VialRGB ids. `0` means "everything
    /// ported", which is what an unconfigured build gets.
    enabled_count: u8,
    enabled: [u16; MAX_LEDS],
}

impl SharedRgbState {
    /// An empty state: no LEDs, everything ported.
    pub const fn new() -> Self {
        Self {
            config: RgbConfig::new_const(),
            max_brightness: super::state::MAX_BRIGHTNESS_CEILING,
            // QMK's `(LED_COUNT + 4) / 5`; corrected once a table is installed.
            process_limit: 1,
            led_count: 0,
            leds: [LedInfo::EMPTY; MAX_LEDS],
            enabled_count: 0,
            enabled: [0; MAX_LEDS],
        }
    }

    /// Install the board's LED table.
    ///
    /// Panics if `leds.len()` exceeds [`MAX_LEDS`], which is a board-definition
    /// error rather than something to absorb at run time.
    pub fn set_leds<const LED_COUNT: usize>(&mut self, table: &super::led_config::LedConfig<LED_COUNT>) {
        assert!(
            LED_COUNT <= MAX_LEDS,
            "board has {LED_COUNT} LEDs; raise SharedRgbState::MAX_LEDS"
        );
        for i in 0..LED_COUNT {
            self.leds[i] = table.info(i);
        }
        for slot in self.leds.iter_mut().skip(LED_COUNT) {
            *slot = LedInfo::EMPTY;
        }
        self.led_count = LED_COUNT as u8;
        self.process_limit = super::engine::default_process_limit(LED_COUNT) as u8;
    }

    /// Install the enabled-effect list.
    pub fn set_enabled(&mut self, enabled: &[u16]) {
        let n = enabled.len().min(MAX_LEDS);
        self.enabled[..n].copy_from_slice(&enabled[..n]);
        self.enabled_count = n as u8;
    }

    /// Set the brightness ceiling.
    pub fn set_max_brightness(&mut self, max_brightness: u8) {
        self.max_brightness = max_brightness;
    }

    /// The run-time configuration.
    #[inline]
    pub const fn config(&self) -> &RgbConfig {
        &self.config
    }

    /// Mutable configuration, for the protocol's setters.
    #[inline]
    pub fn config_mut(&mut self) -> &mut RgbConfig {
        &mut self.config
    }

    /// `RGB_MATRIX_MAXIMUM_BRIGHTNESS`.
    #[inline]
    pub const fn max_brightness(&self) -> u8 {
        self.max_brightness
    }

    /// `RGB_MATRIX_LED_PROCESS_LIMIT`.
    #[inline]
    pub const fn process_limit(&self) -> usize {
        self.process_limit as usize
    }

    /// Number of LEDs in the chain.
    #[inline]
    pub const fn led_count(&self) -> usize {
        self.led_count as usize
    }

    /// LED `index`, or `None` past the end.
    #[inline]
    pub fn led(&self, index: usize) -> Option<LedInfo> {
        if index < self.led_count as usize {
            Some(self.leds[index])
        } else {
            None
        }
    }

    /// LED `index`'s `LED_FLAG_*` bitmap, or `LED_FLAG_NONE` past the end.
    #[inline]
    pub fn led_flags(&self, index: usize) -> u8 {
        self.led(index).map_or(LED_FLAG_NONE, |info| info.flags)
    }

    /// The enabled effects, as VialRGB ids. Empty means "everything ported".
    #[inline]
    pub fn enabled(&self) -> &[u16] {
        &self.enabled[..self.enabled_count as usize]
    }

    /// Apply an HSV change from the protocol, clamping to the ceiling.
    ///
    /// Returns whether anything changed.
    pub fn set_hsv(&mut self, hsv: Hsv) -> bool {
        self.config.set_hsv(hsv.h, hsv.s, hsv.v, self.max_brightness)
    }
}

impl Default for SharedRgbState {
    fn default() -> Self {
        Self::new()
    }
}

/// A shared RGB state behind a mutex.
pub type SharedRgb<M> = Mutex<M, core::cell::RefCell<SharedRgbState>>;

/// Build a shared state with an empty table.
pub const fn new_shared<M: RawMutex>() -> SharedRgb<M> {
    Mutex::new(core::cell::RefCell::new(SharedRgbState::new()))
}

/// The global the protocol and the engine both reach.
///
/// A single instance keeps the two sides in step without threading a reference
/// through `RmkConfig`, which is built before the board's LED table is known.
/// What stays per-instance is the engine's own scratch — the frame buffer, the
/// hit tracker, the RNG — none of which the protocol touches.
pub static RGB_STATE: SharedRgb<crate::RawMutex> = new_shared();

/// Whether RGB support was built in at all.
///
/// The engine task sets this once it has installed its table, so the protocol
/// can answer `GET_INFO` with a "not supported" response instead of reporting a
/// zero-LED keyboard when the feature is present but no board is configured.
static RGB_READY: AtomicU8 = AtomicU8::new(0);

/// Mark the engine as ready to serve the protocol.
pub fn mark_ready() {
    RGB_READY.store(1, Ordering::Release);
}

/// Whether [`mark_ready`] has run.
#[inline]
pub fn is_ready() -> bool {
    RGB_READY.load(Ordering::Acquire) != 0
}

/// A flag the protocol sets when it changes the configuration.
///
/// The VIA `CustomSave` (0x09) handler persists it. Keeping the flag rather than
/// saving on every write matches QMK, where `_noeeprom` setters deliberately do
/// not touch flash — Vial sends many reports while the user drags a slider, and
/// each one must not cost a flash write.
static RGB_DIRTY: AtomicU8 = AtomicU8::new(0);

/// Mark the configuration as changed since the last save.
#[inline]
pub fn mark_dirty() {
    RGB_DIRTY.store(1, Ordering::Release);
}

/// Consume the dirty flag, returning whether a save is needed.
#[inline]
pub fn take_dirty() -> bool {
    RGB_DIRTY.swap(0, Ordering::AcqRel) != 0
}

/// Whether the configuration has changed since the last save.
#[inline]
pub fn is_dirty() -> bool {
    RGB_DIRTY.load(Ordering::Acquire) != 0
}

/// Run `f` with the shared state.
///
/// The closure runs inside a critical section, so it must be short and must not
/// await — the same rule `blocking_mutex` imposes everywhere else in RMK.
pub fn with_state<R>(f: impl FnOnce(&mut SharedRgbState) -> R) -> R {
    RGB_STATE.lock(|cell| f(&mut cell.borrow_mut()))
}

/// A copy of the configuration, for the engine's per-frame work.
pub fn config_snapshot() -> RgbConfig {
    with_state(|s| s.config)
}

/// Read one LED's description, for `GET_LED_INFO`.
pub fn led_info(index: usize) -> Option<LedInfo> {
    with_state(|s| s.led(index))
}

/// The flag set `GET_LED_INFO` reports for an LED, or `LED_FLAG_NONE` past the
/// end.
pub fn led_flags(index: usize) -> u8 {
    led_info(index).map_or(LED_FLAG_NONE, |info| info.flags)
}

/// The value the default flag filter uses.
pub const ALL_FLAGS: u8 = LED_FLAG_ALL;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::led_config::{LedConfig, LedPoint, LED_FLAG_KEYLIGHT, LED_FLAG_MODIFIER};
    use crate::rgb::registry;

    /// The global is shared by all tests in a binary, so these exercise the
    /// value type directly rather than racing on [`RGB_STATE`]. The one test
    /// that does touch the global restores what it changed.
    fn state() -> SharedRgbState {
        SharedRgbState::new()
    }

    fn table<const N: usize>() -> LedConfig<N> {
        let mut points = [LedPoint::new(0, 0); N];
        let mut matrix = [(0u8, 0u8); N];
        for i in 0..N {
            points[i] = LedPoint::new((i as u8) * 10, 32);
            matrix[i] = (0, i as u8);
        }
        LedConfig::new(points, [LED_FLAG_KEYLIGHT; N], matrix)
    }

    #[test]
    fn a_new_state_has_no_leds_and_no_enabled_list() {
        let s = state();
        assert_eq!(s.led_count(), 0);
        assert!(s.enabled().is_empty());
        assert_eq!(s.led(0), None);
        assert_eq!(s.led_flags(0), LED_FLAG_NONE);
        assert_eq!(s.max_brightness(), super::super::state::MAX_BRIGHTNESS_CEILING);
    }

    #[test]
    fn installing_a_table_fills_the_prefix_and_clears_the_tail() {
        let mut s = state();
        let table = table::<5>();
        s.set_leds(&table);
        assert_eq!(s.led_count(), 5);

        for i in 0..5 {
            let info = s.led(i).unwrap();
            assert_eq!(info.point, LedPoint::new((i as u8) * 10, 32));
            assert_eq!(info.matrix_pos, Some((0, i as u8)));
        }
        // Past the live prefix the entries are blanked, so a stale table cannot
        // leak into `GET_LED_INFO`.
        assert_eq!(s.led(5), None);
        assert!(s.leds[5..].iter().all(|e| *e == LedInfo::EMPTY));
    }

    #[test]
    fn installing_a_table_derives_the_process_limit() {
        let mut s = state();
        s.set_leds(&table::<19>());
        // QMK's `(19 + 4) / 5 == 4`.
        assert_eq!(s.process_limit(), 4);
    }

    #[test]
    fn an_unmapped_led_keeps_its_position_but_reports_no_matrix_slot() {
        let mut s = state();
        let leds = LedConfig::new(
            [LedPoint::new(112, 32)],
            [LED_FLAG_MODIFIER],
            [LedConfig::<1>::UNMAPPED],
        );
        s.set_leds(&leds);

        let info = s.led(0).unwrap();
        assert_eq!(info.point, LedPoint::new(112, 32));
        assert_eq!(info.flags, LED_FLAG_MODIFIER);
        assert_eq!(info.matrix_pos, None, "not under a key");
    }

    #[test]
    fn the_enabled_list_round_trips_and_bounds_itself() {
        let mut s = state();
        s.set_enabled(&[2, 6, 13]);
        assert_eq!(s.enabled(), &[2, 6, 13]);

        // An oversized list is truncated rather than overflowing.
        let oversized = [7u16; MAX_LEDS + 4];
        s.set_enabled(&oversized);
        assert_eq!(s.enabled().len(), MAX_LEDS);
        assert!(s.enabled().iter().all(|id| *id == 7));
    }

    #[test]
    fn hsv_is_clamped_to_the_ceiling() {
        let mut s = state();
        s.set_max_brightness(128);
        s.config_mut().enable();
        assert!(s.set_hsv(Hsv::new(10, 20, 255)));
        assert_eq!(s.config().val(), 128, "clamped");
        assert_eq!(s.config().hue(), 10);
    }

    #[test]
    fn set_hsv_reports_whether_anything_changed() {
        let mut s = state();
        s.config_mut().enable();
        assert!(s.set_hsv(Hsv::new(1, 2, 3)));
        assert!(!s.set_hsv(Hsv::new(1, 2, 3)), "same values, no change");
    }

    /// The protocol reaches the same value the engine would render.
    #[test]
    fn the_global_is_usable_from_outside_the_engine() {
        // Save and restore, since the static is shared across the test binary.
        let before = with_state(|s| s.config);
        let before_ready = is_ready();

        with_state(|s| {
            s.set_max_brightness(200);
            s.config_mut().enable();
            s.config_mut().set_mode(24, registry::EFFECT_MAX);
        });
        assert_eq!(config_snapshot().mode(), 24);

        with_state(|s| s.config = before);
        if before_ready {
            mark_ready();
        }
    }

    #[test]
    fn ready_flag_is_a_one_way_latch() {
        // Reading it must be safe before the engine exists.
        let _ = is_ready();
        mark_ready();
        assert!(is_ready());
    }
}
