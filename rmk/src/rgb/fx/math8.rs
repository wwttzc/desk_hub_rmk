//! 8-bit integer math, ported from QMK's `lib/lib8tion/math8.h`,
//! `lib/lib8tion/scale8.h` and the easing/lerp half of `lib/lib8tion/lib8tion.h`.
//!
//! QMK picks AVR or ARM assembly for several of these; we always take the C
//! branch (`*_C == 1`), which the headers provide as the portable fallback and
//! which is what any non-AVR, non-Teensy target compiles to anyway.
//!
//! Two of FastLED's behaviour switches are *not* defined by QMK, so the
//! truncating variants below are the live ones. Porting the "fixed" variants by
//! accident would shift every brightness by one part in 256:
//!
//! - `FASTLED_SCALE8_FIXED` undefined → `scale8(255, 255) == 254`, not 255.
//!
//! Only the helpers the effects actually call are ported; lib8tion's blending,
//! easing and wave generators have no caller in this crate and would be dead
//! code on the target.
//!
//! Everything here is `const fn` where the C original could be folded at
//! compile time, so generated LED tables can reuse it.

/// `scale8`: `i * (scale / 256)`, truncating.
///
/// The odd-looking `255 * 255 → 254` is QMK's actual behaviour, not a bug in
/// this port (see the module doc).
#[inline]
pub const fn scale8(i: u8, scale: u8) -> u8 {
    ((i as u16 * scale as u16) >> 8) as u8
}

/// `scale16by8`: `i * (scale / 256)` for a 16-bit input.
#[inline]
pub const fn scale16by8(i: u16, scale: u8) -> u16 {
    ((i as u32 * scale as u32) >> 8) as u16
}

/// `scale16`: `i * (scale / 65536)`.
#[inline]
pub const fn scale16(i: u16, scale: u16) -> u16 {
    ((i as u32 * scale as u32) / 65536) as u16
}

/// `qadd8`: add, saturating at 255.
#[inline]
pub const fn qadd8(i: u8, j: u8) -> u8 {
    let t = i as u16 + j as u16;
    if t > 255 { 255 } else { t as u8 }
}

/// `qadd7`: add two signed 7-bit values, saturating at 127.
#[inline]
pub const fn qadd7(i: i8, j: i8) -> i8 {
    let t = i as i16 + j as i16;
    if t > 127 { 127 } else { t as i8 }
}

/// `qsub8`: subtract, flooring at 0.
#[inline]
pub const fn qsub8(i: u8, j: u8) -> u8 {
    let t = i as i16 - j as i16;
    if t < 0 { 0 } else { t as u8 }
}

/// `mod8`: `a % m` by repeated subtraction.
///
/// `m == 0` would spin forever, exactly as in C; QMK only ever passes a
/// non-zero effect count. No assertion here: this crate remaps `debug_assert!`
/// to `defmt`'s non-const version, which cannot appear in a `const fn`.
#[inline]
pub const fn mod8(a: u8, m: u8) -> u8 {
    let mut a = a;
    while a >= m {
        a -= m;
    }
    a
}

/// `abs8`.
#[inline]
pub const fn abs8(i: i8) -> i8 {
    i.wrapping_abs()
}

/// `sqrt16`: integer square root of a 16-bit value, by bisection.
#[inline]
pub const fn sqrt16(x: u16) -> u8 {
    if x <= 1 {
        return x as u8;
    }

    let mut low: u8 = 1;
    let mut hi: u8 = if x > 7904 { 255 } else { ((x >> 5) + 8) as u8 };

    loop {
        let mid = ((low as u16 + hi as u16) >> 1) as u8;
        if (mid as u16 * mid as u16) > x {
            hi = mid.wrapping_sub(1);
        } else {
            if mid == 255 {
                return 255;
            }
            low = mid + 1;
        }
        if hi < low {
            break;
        }
    }

    low - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single most load-bearing semantic in the whole port: QMK does not
    /// define `FASTLED_SCALE8_FIXED`, so this truncates. If the port ever
    /// "fixes" it to 255, every colour and brightness drifts by one.
    #[test]
    fn scale8_truncates_like_qmk() {
        assert_eq!(scale8(255, 255), 254);
        assert_eq!(scale8(0, 255), 0);
        assert_eq!(scale8(255, 0), 0);
        assert_eq!(scale8(100, 128), 50);
        assert_eq!(scale8(128, 128), 64);
    }

    #[test]
    fn scale16by8_and_scale16() {
        assert_eq!(scale16by8(65535, 255), 65279);
        assert_eq!(scale16by8(1024, 128), 512);
        assert_eq!(scale16(65535, 65535), 65534);
        assert_eq!(scale16(65535, 0), 0);
    }

    #[test]
    fn saturating_arithmetic() {
        assert_eq!(qadd8(200, 100), 255);
        assert_eq!(qadd8(10, 20), 30);
        assert_eq!(qsub8(10, 20), 0);
        assert_eq!(qsub8(30, 20), 10);
        assert_eq!(abs8(-5), 5);
        assert_eq!(abs8(5), 5);
    }

    #[test]
    fn modulus_helper_matches_the_closed_form() {
        for a in 0..=255u8 {
            for m in 1..=17u8 {
                assert_eq!(mod8(a, m), a % m, "mod8({a}, {m})");
            }
        }
    }

    #[test]
    fn sqrt16_matches_floor_sqrt() {
        for x in 0..=1024u16 {
            assert_eq!(sqrt16(x) as u32, (x as f64).sqrt() as u32, "sqrt16({x})");
        }
        // Upper region: the bisection seeds hi = 255 above this threshold.
        for x in [7904u16, 7905, 10000, 40000, 65025, 65535] {
            assert_eq!(sqrt16(x) as u32, (x as f64).sqrt() as u32, "sqrt16({x})");
        }
    }
}
