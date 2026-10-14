//! RGB Matrix run-time state: the port of QMK's `rgb_config_t` plus the
//! effect-switching rules that act on it.
//!
//! QMK packs this into a `uint64_t` that lives in EEPROM, with `enable` as a
//! 2-bit field and `mode` as 6 bits. We keep the same field widths, because
//! the packing is observable: it caps the effect count at 64 and the Vial
//! protocol reads these fields back out.
//!
//! Everything here is a plain value type. It does not know about a timer, an
//! LED chain, or flash — persisting it is [`crate::rgb::storage`]'s job.

use serde::{Deserialize, Serialize};

use super::fx::color::Hsv;

/// QMK's `RGB_MATRIX_MAXIMUM_BRIGHTNESS` default, and the ceiling the Vial
/// protocol enforces (it redefines the macro to `UINT8_MAX` if a keyboard asks
/// for more).
pub const MAX_BRIGHTNESS_CEILING: u8 = 255;

/// `RGB_MATRIX_DEFAULT_ON`.
pub const DEFAULT_ON: bool = true;
/// `RGB_MATRIX_DEFAULT_HUE`.
pub const DEFAULT_HUE: u8 = 0;
/// `RGB_MATRIX_DEFAULT_SAT`.
pub const DEFAULT_SAT: u8 = 255;
/// `RGB_MATRIX_DEFAULT_SPD`.
pub const DEFAULT_SPD: u8 = 255 / 2;
/// `RGB_MATRIX_DEFAULT_FLAGS`.
pub const DEFAULT_FLAGS: u8 = super::led_config::LED_FLAG_ALL;

/// `RGB_MATRIX_HUE_STEP`.
pub const HUE_STEP: u8 = 8;
/// `RGB_MATRIX_SAT_STEP`.
pub const SAT_STEP: u8 = 16;
/// `RGB_MATRIX_VAL_STEP`.
pub const VAL_STEP: u8 = 16;
/// `RGB_MATRIX_SPD_STEP`.
pub const SPD_STEP: u8 = 16;

/// `RGB_MATRIX_FLAG_STEPS`: the flag combinations the cycling keys walk.
pub const DEFAULT_FLAG_STEPS: [u8; 4] = [
    super::led_config::LED_FLAG_ALL,
    super::led_config::LED_FLAG_KEYLIGHT | super::led_config::LED_FLAG_MODIFIER,
    super::led_config::LED_FLAG_UNDERGLOW,
    super::led_config::LED_FLAG_NONE,
];

/// The run-time RGB configuration, QMK's `rgb_config_t`.
///
/// `mode` is stored as the *enabled effect's* id; `0` has no meaning here
/// because effect ids start at 1 in QMK (`RGB_MATRIX_NONE = 0` is "off", and
/// the driver is expected to be disabled when `enable` is false).
///
/// The field order and widths mirror the C bitfield so that a serialized value
/// stays compact and comparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct RgbConfig {
    /// Whether the LEDs are on at all (`rgb_config_t::enable`, 2 bits in C).
    pub enable: bool,
    /// The active effect id (`rgb_config_t::mode`, 6 bits in C — which is why
    /// QMK tops out at 64 effects).
    ///
    /// Ids are [`crate::rgb::registry`]'s, i.e. VialRGB's numbering: `0` means
    /// off, `1` means host-driven direct colour, and real effects start at 2.
    pub mode: u16,
    /// Global colour.
    pub hsv: Hsv,
    /// Animation speed, `0..=255`. Each effect interprets it its own way.
    pub speed: u8,
    /// Active `LED_FLAG_*` filter.
    pub flags: u8,
}

impl Default for RgbConfig {
    /// `RGB_MATRIX_DEFAULT_*`, with the board's brightness ceiling applied to
    /// the default value rather than QMK's compile-time constant.
    fn default() -> Self {
        Self::with_defaults(DEFAULT_ON, DEFAULT_HUE, DEFAULT_SAT, DEFAULT_SPD, DEFAULT_FLAGS, MAX_BRIGHTNESS_CEILING)
    }
}

impl RgbConfig {
    /// The `RGB_MATRIX_DEFAULT_*` set, as `eeconfig_update_rgb_matrix_default`
    /// writes it.
    ///
    /// `val` starts at the brightness ceiling, as QMK's
    /// `RGB_MATRIX_DEFAULT_VAL` is defined in terms of
    /// `RGB_MATRIX_MAXIMUM_BRIGHTNESS`.
    pub const fn with_defaults(
        enable: bool,
        hue: u8,
        sat: u8,
        speed: u8,
        flags: u8,
        max_brightness: u8,
    ) -> Self {
        Self {
            enable,
            // The first selectable effect. `set_mode` clamps an out-of-range
            // mode onto the table's ends, which `registry` owns.
            mode: super::registry::EFFECT_FIRST,
            hsv: Hsv::new(hue, sat, max_brightness),
            speed,
            flags,
        }
    }

    /// `rgb_matrix_is_enabled`.
    #[inline]
    pub const fn is_enabled(&self) -> bool {
        self.enable
    }

    /// `rgb_matrix_enable_noeeprom`.
    #[inline]
    pub fn enable(&mut self) {
        self.enable = true;
    }

    /// `rgb_matrix_disable_noeeprom`.
    #[inline]
    pub fn disable(&mut self) {
        self.enable = false;
    }

    /// `rgb_matrix_toggle_noeeprom`.
    #[inline]
    pub fn toggle(&mut self) {
        self.enable = !self.enable;
    }

    /// `rgb_matrix_get_mode`.
    #[inline]
    pub const fn mode(&self) -> u16 {
        self.mode
    }

    /// `rgb_matrix_mode_eeprom_helper`'s clamping.
    ///
    /// QMK refuses to change the mode while disabled, clamps a mode below the
    /// first effect up to it, and clamps one at or past the effect count down to
    /// the last. `effect_max` is `RGB_MATRIX_EFFECT_MAX`.
    ///
    /// The range ends come from [`crate::rgb::registry`], so the clamps track the
    /// effect table instead of a hard-coded 1.
    ///
    /// Returns `true` when the mode actually changed, which the caller uses to
    /// decide whether a frame must be re-rendered from scratch.
    pub fn set_mode(&mut self, mode: u16, effect_max: u16) -> bool {
        if !self.enable {
            return false;
        }
        let first = super::registry::EFFECT_FIRST;
        let clamped = if mode < first {
            first
        } else if mode >= effect_max {
            effect_max.saturating_sub(1)
        } else {
            mode
        };
        let changed = clamped != self.mode;
        self.mode = clamped;
        changed
    }

    /// Overwrite the mode without clamping or the enabled check.
    ///
    /// [`set_mode`](Self::set_mode) intentionally clamps onto the effect table's
    /// range, which is right for every real effect but wrong for the two
    /// sentinels that sit outside it: `OFF` (0) and `DIRECT` (1). This is the
    /// escape hatch those two use, so their handling stays in one place.
    pub(crate) fn set_mode_raw(&mut self, mode: u16) {
        self.mode = mode;
    }

    /// `rgb_matrix_step_helper`: the next effect, wrapping back to the first.
    ///
    /// QMK walks the raw enum, not just the enabled subset; an effect that was
    /// compiled out simply has no implementation and falls back, so the subset
    /// that matters is the one the user enabled at build time.
    pub fn step_mode(&mut self, effect_max: u16) -> bool {
        let next = self.mode.wrapping_add(1);
        let next = if next < effect_max {
            next
        } else {
            super::registry::EFFECT_FIRST
        };
        self.set_mode(next, effect_max)
    }

    /// `rgb_matrix_step_reverse_helper`.
    pub fn step_mode_reverse(&mut self, effect_max: u16) -> bool {
        let first = super::registry::EFFECT_FIRST;
        let prev = self.mode.wrapping_sub(1);
        let prev = if prev < first {
            effect_max.saturating_sub(1)
        } else {
            prev
        };
        self.set_mode(prev, effect_max)
    }

    /// `rgb_matrix_sethsv_eeprom_helper`: sets HSV, clamping brightness to the
    /// board ceiling and refusing while disabled.
    ///
    /// Returns `true` when a value changed.
    pub fn set_hsv(&mut self, h: u8, s: u8, v: u8, max_brightness: u8) -> bool {
        if !self.enable {
            return false;
        }
        let next = Hsv::new(h, s, v.min(max_brightness));
        let changed = next != self.hsv;
        self.hsv = next;
        changed
    }

    /// `rgb_matrix_get_hsv`.
    #[inline]
    pub const fn hsv(&self) -> Hsv {
        self.hsv
    }

    /// `rgb_matrix_get_hue`.
    #[inline]
    pub const fn hue(&self) -> u8 {
        self.hsv.h
    }

    /// `rgb_matrix_get_sat`.
    #[inline]
    pub const fn sat(&self) -> u8 {
        self.hsv.s
    }

    /// `rgb_matrix_get_val`.
    #[inline]
    pub const fn val(&self) -> u8 {
        self.hsv.v
    }

    /// `rgb_matrix_increase_hue_helper`: hue wraps around the byte range.
    pub fn increase_hue(&mut self, max_brightness: u8) -> bool {
        let h = self.hsv.h.wrapping_add(HUE_STEP);
        self.set_hsv(h, self.hsv.s, self.hsv.v, max_brightness)
    }

    /// `rgb_matrix_decrease_hue_helper`.
    pub fn decrease_hue(&mut self, max_brightness: u8) -> bool {
        let h = self.hsv.h.wrapping_sub(HUE_STEP);
        self.set_hsv(h, self.hsv.s, self.hsv.v, max_brightness)
    }

    /// `rgb_matrix_increase_sat_helper`: saturating at 255.
    pub fn increase_sat(&mut self, max_brightness: u8) -> bool {
        let s = super::fx::math8::qadd8(self.hsv.s, SAT_STEP);
        self.set_hsv(self.hsv.h, s, self.hsv.v, max_brightness)
    }

    /// `rgb_matrix_decrease_sat_helper`.
    pub fn decrease_sat(&mut self, max_brightness: u8) -> bool {
        let s = super::fx::math8::qsub8(self.hsv.s, SAT_STEP);
        self.set_hsv(self.hsv.h, s, self.hsv.v, max_brightness)
    }

    /// `rgb_matrix_increase_val_helper`.
    pub fn increase_val(&mut self, max_brightness: u8) -> bool {
        let v = super::fx::math8::qadd8(self.hsv.v, VAL_STEP);
        self.set_hsv(self.hsv.h, self.hsv.s, v, max_brightness)
    }

    /// `rgb_matrix_decrease_val_helper`.
    pub fn decrease_val(&mut self, max_brightness: u8) -> bool {
        let v = super::fx::math8::qsub8(self.hsv.v, VAL_STEP);
        self.set_hsv(self.hsv.h, self.hsv.s, v, max_brightness)
    }

    /// `rgb_matrix_get_speed`.
    #[inline]
    pub const fn speed(&self) -> u8 {
        self.speed
    }

    /// `rgb_matrix_set_speed_eeprom_helper`. Unlike the HSV setters this one
    /// is not gated on `enable`.
    pub fn set_speed(&mut self, speed: u8) -> bool {
        let changed = speed != self.speed;
        self.speed = speed;
        changed
    }

    /// `rgb_matrix_increase_speed_helper`.
    pub fn increase_speed(&mut self) -> bool {
        let s = super::fx::math8::qadd8(self.speed, SPD_STEP);
        self.set_speed(s)
    }

    /// `rgb_matrix_decrease_speed_helper`.
    pub fn decrease_speed(&mut self) -> bool {
        let s = super::fx::math8::qsub8(self.speed, SPD_STEP);
        self.set_speed(s)
    }

    /// `rgb_matrix_get_flags`.
    #[inline]
    pub const fn flags(&self) -> u8 {
        self.flags
    }

    /// `rgb_matrix_set_flags_eeprom_helper`.
    pub fn set_flags(&mut self, flags: u8) -> bool {
        let changed = flags != self.flags;
        self.flags = flags;
        changed
    }

    /// `rgb_matrix_flags_step_helper`: advance to the next entry of
    /// `RGB_MATRIX_FLAG_STEPS`, wrapping. An unknown current value restarts
    /// the cycle, as QMK's `next = 0` fallback does.
    pub fn step_flags(&mut self, steps: &[u8]) -> bool {
        self.set_flags(step_flags(self.flags, steps, true))
    }

    /// `rgb_matrix_flags_step_reverse_helper`.
    pub fn step_flags_reverse(&mut self, steps: &[u8]) -> bool {
        self.set_flags(step_flags(self.flags, steps, false))
    }
}

/// The index walk shared by [`RgbConfig::step_flags`] and its reverse.
fn step_flags(current: u8, steps: &[u8], forward: bool) -> u8 {
    if steps.is_empty() {
        return current;
    }
    let last = steps.len() - 1;
    let mut next = 0usize;
    for (i, candidate) in steps.iter().enumerate() {
        if *candidate == current {
            next = if forward {
                if i == last { 0 } else { i + 1 }
            } else if i == 0 {
                last
            } else {
                i - 1
            };
            break;
        }
    }
    steps[next]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::led_config::{LED_FLAG_ALL, LED_FLAG_NONE, LED_FLAG_UNDERGLOW};

    /// A stand-in for `RGB_MATRIX_EFFECT_MAX` from a build with only a few
    /// effects enabled. Deliberately smaller than the real table so the clamps
    /// are exercised at a boundary the tests control.
    const EFFECT_MAX: u16 = 10;

    #[test]
    fn defaults_match_qmk() {
        let config = RgbConfig::default();
        assert!(config.is_enabled());
        assert_eq!(config.hue(), DEFAULT_HUE);
        assert_eq!(config.sat(), DEFAULT_SAT);
        assert_eq!(config.speed(), DEFAULT_SPD);
        assert_eq!(config.flags(), DEFAULT_FLAGS);
        assert_eq!(
            config.mode(),
            crate::rgb::registry::EFFECT_FIRST,
            "mode starts at the first selectable effect"
        );
    }

    #[test]
    fn default_brightness_uses_the_board_ceiling() {
        // A board that caps at 128 must not boot at 255.
        let config = RgbConfig::with_defaults(true, 0, 255, 127, LED_FLAG_ALL, 128);
        assert_eq!(config.val(), 128);
    }

    #[test]
    fn mode_clamping_matches_qmk() {
        use crate::rgb::registry::{EFFECT_FIRST, EFFECT_MAX};

        let mut config = RgbConfig::default();

        assert!(!config.set_mode(0, EFFECT_MAX), "0 clamps up to the first effect, already there");
        assert_eq!(config.mode(), EFFECT_FIRST);

        assert!(config.set_mode(6, EFFECT_MAX));
        assert_eq!(config.mode(), 6);

        // At or past the effect count clamps to the last effect.
        assert!(config.set_mode(EFFECT_MAX, EFFECT_MAX));
        assert_eq!(config.mode(), EFFECT_MAX - 1);
        assert!(!config.set_mode(99, EFFECT_MAX), "no change, already clamped");
    }

    #[test]
    fn mode_changes_are_refused_while_disabled() {
        use crate::rgb::registry::{EFFECT_FIRST, EFFECT_MAX};

        let mut config = RgbConfig::default();
        config.disable();
        assert!(!config.set_mode(4, EFFECT_MAX));
        assert_eq!(config.mode(), EFFECT_FIRST, "the mode is untouched");
        config.enable();
        assert!(config.set_mode(4, EFFECT_MAX));
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        use crate::rgb::registry::{EFFECT_FIRST, EFFECT_MAX};

        let mut config = RgbConfig::default();

        config.set_mode(EFFECT_MAX - 1, EFFECT_MAX);
        assert!(config.step_mode(EFFECT_MAX));
        assert_eq!(config.mode(), EFFECT_FIRST, "past the last effect wraps to the first");

        config.set_mode(EFFECT_FIRST, EFFECT_MAX);
        assert!(config.step_mode_reverse(EFFECT_MAX));
        assert_eq!(config.mode(), EFFECT_MAX - 1, "before the first wraps to the last");
    }

    #[test]
    fn hsv_is_clamped_and_gated_on_enable() {
        let mut config = RgbConfig::default();

        config.set_hsv(10, 20, 255, 128);
        assert_eq!(config.hsv(), Hsv::new(10, 20, 128), "brightness clamps to the ceiling");

        config.disable();
        assert!(!config.set_hsv(99, 99, 99, 128), "refused while disabled");
        assert_eq!(config.hue(), 10, "unchanged");
    }

    #[test]
    fn hsv_steps_saturate_and_wrap_like_qmk() {
        let mut config = RgbConfig::default();

        config.set_hsv(0, 250, 250, 255);
        config.increase_sat(255);
        assert_eq!(config.sat(), 255, "qadd8 saturates");
        config.increase_val(255);
        assert_eq!(config.val(), 255);

        config.set_hsv(0, 5, 5, 255);
        config.decrease_sat(255);
        assert_eq!(config.sat(), 0, "qsub8 floors");
        config.decrease_val(255);
        assert_eq!(config.val(), 0);

        // Hue wraps rather than clamping: 250 + 8 crosses the byte.
        config.set_hsv(250, 100, 100, 255);
        config.increase_hue(255);
        assert_eq!(config.hue(), 2);
        config.decrease_hue(255);
        assert_eq!(config.hue(), 250, "and wraps back below zero");
    }

    #[test]
    fn speed_is_not_gated_on_enable() {
        let mut config = RgbConfig::default();
        config.disable();
        assert!(config.set_speed(200), "QMK's speed setter has no enable check");
        assert_eq!(config.speed(), 200);
    }

    #[test]
    fn speed_steps_saturate() {
        let mut config = RgbConfig::default();
        config.set_speed(250);
        config.increase_speed();
        assert_eq!(config.speed(), 255);
        config.set_speed(5);
        config.decrease_speed();
        assert_eq!(config.speed(), 0);
    }

    #[test]
    fn flag_stepping_walks_the_table_both_ways() {
        use crate::rgb::led_config::{LED_FLAG_KEYLIGHT, LED_FLAG_MODIFIER};

        let mut config = RgbConfig::default();

        // The default table is ALL -> KEYLIGHT|MODIFIER -> UNDERGLOW -> NONE.
        assert_eq!(config.flags(), LED_FLAG_ALL);
        config.step_flags(&DEFAULT_FLAG_STEPS);
        assert_eq!(config.flags(), LED_FLAG_KEYLIGHT | LED_FLAG_MODIFIER);
        config.step_flags(&DEFAULT_FLAG_STEPS);
        assert_eq!(config.flags(), LED_FLAG_UNDERGLOW);
        config.step_flags(&DEFAULT_FLAG_STEPS);
        assert_eq!(config.flags(), LED_FLAG_NONE);
        config.step_flags(&DEFAULT_FLAG_STEPS);
        assert_eq!(config.flags(), LED_FLAG_ALL, "wraps to the start");

        config.step_flags_reverse(&DEFAULT_FLAG_STEPS);
        assert_eq!(config.flags(), LED_FLAG_NONE, "and backwards from the start");
    }

    #[test]
    fn flag_stepping_restarts_on_an_unknown_value() {
        let mut config = RgbConfig::default();
        config.set_flags(0x42);
        assert!(config.step_flags(&DEFAULT_FLAG_STEPS));
        assert_eq!(config.flags(), LED_FLAG_ALL, "QMK's `next = 0` fallback");
    }

    #[test]
    fn flag_stepping_tolerates_an_empty_table() {
        let mut config = RgbConfig::default();
        assert!(!config.step_flags(&[]));
        assert_eq!(config.flags(), LED_FLAG_ALL);
    }
}
