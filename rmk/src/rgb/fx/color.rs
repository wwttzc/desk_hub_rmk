//! Colour types and conversion, ported from QMK's `quantum/color.h` and
//! `quantum/color.c`.
//!
//! QMK's HSV is byte-ranged: **hue is `0..=255`, not degrees**, and both
//! saturation and value are `0..=255`. Effects rely on the byte wrap (for
//! example `hsv.h = h - time`), so keep the wrapping arithmetic at call sites.
//!
//! `USE_CIE1931_CURVE` is not defined by QMK builds, so the CIE gamma path in
//! `hsv_to_rgb_impl` is not ported; `hsv_to_rgb` here is QMK's
//! `hsv_to_rgb_nocie`.

use serde::{Deserialize, Serialize};

/// An HSV colour with QMK's byte ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Hsv {
    /// Hue, `0..=255` for a full turn.
    pub h: u8,
    /// Saturation, `0..=255`.
    pub s: u8,
    /// Value (brightness), `0..=255`.
    pub v: u8,
}

impl Hsv {
    #[inline]
    pub const fn new(h: u8, s: u8, v: u8) -> Self {
        Self { h, s, v }
    }

    /// Shorthand for the common "keep hue and saturation, change brightness".
    #[inline]
    pub const fn with_v(self, v: u8) -> Self {
        Self { v, ..self }
    }

    /// Shorthand for the common "keep brightness, change hue".
    #[inline]
    pub const fn with_h(self, h: u8) -> Self {
        Self { h, ..self }
    }

    /// Convert to RGB, as `hsv_to_rgb` does.
    #[inline]
    pub const fn to_rgb(self) -> Rgb {
        hsv_to_rgb(self)
    }
}

/// An RGB colour, one byte per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const RED: Self = Self { r: 255, g: 0, b: 0 };
    pub const GREEN: Self = Self { r: 0, g: 255, b: 0 };
    pub const BLUE: Self = Self { r: 0, g: 0, b: 255 };
    pub const WHITE: Self = Self { r: 255, g: 255, b: 255 };

    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// `hsv_to_rgb`: QMK's integer HSV→RGB.
///
/// Reproduces the C original's intermediate truncations, including the
/// `region`/`remainder` byte math, so results are bit-identical.
#[inline]
pub const fn hsv_to_rgb(hsv: Hsv) -> Rgb {
    let s = hsv.s as u32;
    let v = hsv.v as u32;

    // Saturation zero means grey at the current brightness.
    if s == 0 {
        return Rgb {
            r: v as u8,
            g: v as u8,
            b: v as u8,
        };
    }

    let h = hsv.h as u32;

    // `region` and `remainder` are `uint8_t` in C, so they truncate.
    let region = (h * 6 / 255) as u8;
    let remainder = ((h * 2).wrapping_sub(region as u32 * 85) * 3) as u8;
    let remainder = remainder as u32;

    let p = ((v * (255 - s)) >> 8) as u8;
    let q = ((v * (255 - ((s * remainder) >> 8))) >> 8) as u8;
    let t = ((v * (255 - ((s * (255 - remainder)) >> 8))) >> 8) as u8;
    let v = v as u8;

    match region {
        0 | 6 => Rgb { r: v, g: t, b: p },
        1 => Rgb { r: q, g: v, b: p },
        2 => Rgb { r: p, g: v, b: t },
        3 => Rgb { r: p, g: q, b: v },
        4 => Rgb { r: t, g: p, b: v },
        _ => Rgb { r: v, g: p, b: q },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_hues_land_on_the_primaries() {
        // Hue 0 is red, and 255 wraps back to the same region.
        let red = hsv_to_rgb(Hsv::new(0, 255, 255));
        assert_eq!(red, Rgb::RED);
        assert_eq!(hsv_to_rgb(Hsv::new(255, 255, 255)), Rgb::RED);

        // 1/3 turn is green, 2/3 turn is blue (85 and 170 are the byte steps).
        let green = hsv_to_rgb(Hsv::new(85, 255, 255));
        assert_eq!(green.g, 255);
        assert_eq!(green.r, 0);
        assert_eq!(green.b, 0);

        let blue = hsv_to_rgb(Hsv::new(170, 255, 255));
        assert_eq!(blue.b, 255);
        assert_eq!(blue.r, 0);
        assert_eq!(blue.g, 0);
    }

    #[test]
    fn zero_saturation_is_grey_not_black() {
        assert_eq!(hsv_to_rgb(Hsv::new(123, 0, 200)), Rgb::new(200, 200, 200));
        assert_eq!(hsv_to_rgb(Hsv::new(0, 0, 0)), Rgb::BLACK);
    }

    #[test]
    fn zero_value_is_black_for_every_hue() {
        for h in 0..=255u8 {
            assert_eq!(hsv_to_rgb(Hsv::new(h, 255, 0)), Rgb::BLACK, "hue {h}");
        }
    }

    /// Every combination must stay in range; the C code relies on byte
    /// truncation that a naive `u8` port would turn into a panic in debug.
    #[test]
    fn all_inputs_are_representable() {
        for h in (0..=255u8).step_by(7) {
            for s in (0..=255u8).step_by(11) {
                for v in (0..=255u8).step_by(13) {
                    let _ = hsv_to_rgb(Hsv::new(h, s, v));
                }
            }
        }
    }

    #[test]
    fn modifiers_keep_the_other_components() {
        let base = Hsv::new(10, 20, 30);
        assert_eq!(base.with_v(99), Hsv::new(10, 20, 99));
        assert_eq!(base.with_h(99), Hsv::new(99, 20, 30));
    }
}
