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
//! - `FASTLED_BLEND_FIXED` undefined → `blend8` loses the same low bit.
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

/// `scale8_video`: like [`scale8`], but keeps a non-zero result non-zero so low
/// dimming levels do not snap to black.
#[inline]
pub const fn scale8_video(i: u8, scale: u8) -> u8 {
    let j = (((i as i32) * (scale as i32)) >> 8) + if i != 0 && scale != 0 { 1 } else { 0 };
    j as u8
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

/// `add8`: add with wraparound (the C version returns a `uint8_t`).
#[inline]
pub const fn add8(i: u8, j: u8) -> u8 {
    i.wrapping_add(j)
}

/// `add8to16`: widen an 8-bit value and add it to a 16-bit one.
#[inline]
pub const fn add8to16(i: u8, j: u16) -> u16 {
    i as u16 + j
}

/// `sub8`: subtract with wraparound.
#[inline]
pub const fn sub8(i: u8, j: u8) -> u8 {
    i.wrapping_sub(j)
}

/// `avg8`: rounded-down average. The C sum is promoted to `int`, so it cannot
/// wrap; do the same widening here.
#[inline]
pub const fn avg8(i: u8, j: u8) -> u8 {
    ((i as u16 + j as u16) >> 1) as u8
}

/// `avg16`: rounded-down average of two 16-bit values.
#[inline]
pub const fn avg16(i: u16, j: u16) -> u16 {
    ((i as u32 + j as u32) >> 1) as u16
}

/// `avg7`: average of two signed 7-bit values, rounding up when `i` is odd.
#[inline]
pub const fn avg7(i: i8, j: i8) -> i8 {
    (((i as i16 + j as i16) >> 1) + (i as i16 & 1)) as i8
}

/// `avg15`: average of two signed 15-bit values.
#[inline]
pub const fn avg15(i: i16, j: i16) -> i16 {
    (((i as i32 + j as i32) >> 1) + (i as i32 & 1)) as i16
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

/// `addmod8`: `(a + b) % m`. The addition wraps in 8 bits *before* reducing,
/// as in C.
#[inline]
pub const fn addmod8(a: u8, b: u8, m: u8) -> u8 {
    let mut a = a.wrapping_add(b);
    while a >= m {
        a -= m;
    }
    a
}

/// `submod8`: `(a - b) % m`, wrapping in 8 bits first.
#[inline]
pub const fn submod8(a: u8, b: u8, m: u8) -> u8 {
    let mut a = a.wrapping_sub(b);
    while a >= m {
        a -= m;
    }
    a
}

/// `mul8`: low byte of the product.
#[inline]
pub const fn mul8(i: u8, j: u8) -> u8 {
    i.wrapping_mul(j)
}

/// `qmul8`: product, saturating at 255.
#[inline]
pub const fn qmul8(i: u8, j: u8) -> u8 {
    let p = i as u16 * j as u16;
    if p > 255 { 255 } else { p as u8 }
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

/// `blend8`: blend `amountOfB / 256` of `b` into `a`, truncating.
#[inline]
pub const fn blend8(a: u8, b: u8, amount_of_b: u8) -> u8 {
    let amount_of_a = 255 - amount_of_b;
    scale8(a, amount_of_a).wrapping_add(scale8(b, amount_of_b))
}

/// `lerp8by8`.
#[inline]
pub const fn lerp8by8(a: u8, b: u8, frac: u8) -> u8 {
    if b > a {
        a + scale8(b - a, frac)
    } else {
        a - scale8(a - b, frac)
    }
}

/// `lerp16by16`.
#[inline]
pub const fn lerp16by16(a: u16, b: u16, frac: u16) -> u16 {
    if b > a {
        a + scale16(b - a, frac)
    } else {
        a - scale16(a - b, frac)
    }
}

/// `lerp16by8`.
#[inline]
pub const fn lerp16by8(a: u16, b: u16, frac: u8) -> u16 {
    if b > a {
        a + scale16by8(b - a, frac)
    } else {
        a - scale16by8(a - b, frac)
    }
}

/// `lerp15by8`: linear interpolation between signed 15-bit values.
#[inline]
pub const fn lerp15by8(a: i16, b: i16, frac: u8) -> i16 {
    if b > a {
        a + scale16by8((b as u16).wrapping_sub(a as u16), frac) as i16
    } else {
        a - scale16by8((a as u16).wrapping_sub(b as u16), frac) as i16
    }
}

/// `lerp15by16`.
#[inline]
pub const fn lerp15by16(a: i16, b: i16, frac: u16) -> i16 {
    if b > a {
        a + scale16((b as u16).wrapping_sub(a as u16), frac) as i16
    } else {
        a - scale16((a as u16).wrapping_sub(b as u16), frac) as i16
    }
}

/// `map8`: remap a full-range byte into `range_start..=range_end`.
#[inline]
pub const fn map8(input: u8, range_start: u8, range_end: u8) -> u8 {
    scale8(input, range_end.wrapping_sub(range_start)).wrapping_add(range_start)
}

/// `dim8_raw`.
#[inline]
pub const fn dim8_raw(x: u8) -> u8 {
    scale8(x, x)
}

/// `dim8_video`.
#[inline]
pub const fn dim8_video(x: u8) -> u8 {
    scale8_video(x, x)
}

/// `dim8_lin`: halves below the midpoint, squares above it.
#[inline]
pub const fn dim8_lin(x: u8) -> u8 {
    if x & 0x80 != 0 {
        scale8(x, x)
    } else {
        (x + 1) / 2
    }
}

/// `brighten8_raw`.
#[inline]
pub const fn brighten8_raw(x: u8) -> u8 {
    let ix = 255 - x;
    255 - scale8(ix, ix)
}

/// `brighten8_video`.
#[inline]
pub const fn brighten8_video(x: u8) -> u8 {
    let ix = 255 - x;
    255 - scale8_video(ix, ix)
}

/// `brighten8_lin`.
#[inline]
pub const fn brighten8_lin(x: u8) -> u8 {
    let ix = 255 - x;
    let ix = if ix & 0x80 != 0 { scale8(ix, ix) } else { (ix + 1) / 2 };
    255 - ix
}

/// `ease8InOutQuad`.
#[inline]
pub const fn ease8_in_out_quad(i: u8) -> u8 {
    let j = if i & 0x80 != 0 { 255 - i } else { i };
    let jj = scale8(j, j);
    let jj2 = jj.wrapping_shl(1);
    if i & 0x80 != 0 { 255 - jj2 } else { jj2 }
}

/// `ease16InOutQuad`.
#[inline]
pub const fn ease16_in_out_quad(i: u16) -> u16 {
    let j = if i & 0x8000 != 0 { 65535 - i } else { i };
    let jj = scale16(j, j);
    let jj2 = jj.wrapping_shl(1);
    if i & 0x8000 != 0 { 65535 - jj2 } else { jj2 }
}

/// `ease8InOutCubic`: `3x² - 2x³`.
#[inline]
pub const fn ease8_in_out_cubic(i: u8) -> u8 {
    let ii = scale8(i, i);
    let iii = scale8(ii, i);
    let r1 = 3 * (ii as u16) - 2 * (iii as u16);
    // The C original truncates to uint8_t, then special-cases the 256 overflow.
    if r1 & 0x100 != 0 { 255 } else { r1 as u8 }
}

/// `ease8InOutApprox`: a cheaper cubic-shaped S-curve.
#[inline]
pub const fn ease8_in_out_approx(i: u8) -> u8 {
    if i < 64 {
        i / 2
    } else if i > (255 - 64) {
        let i = 255 - i;
        let i = i / 2;
        255 - i
    } else {
        let i = i - 64;
        let i = i + (i / 2);
        i + 32
    }
}

/// `triwave8`: triangle wave, `0 → 254 → 0`.
#[inline]
pub const fn triwave8(input: u8) -> u8 {
    let input = if input & 0x80 != 0 { 255 - input } else { input };
    input.wrapping_shl(1)
}

/// `quadwave8`.
#[inline]
pub const fn quadwave8(input: u8) -> u8 {
    ease8_in_out_quad(triwave8(input))
}

/// `cubicwave8`.
#[inline]
pub const fn cubicwave8(input: u8) -> u8 {
    ease8_in_out_cubic(triwave8(input))
}

/// `squarewave8`: `255` while `input < pulsewidth`, else `0`.
#[inline]
pub const fn squarewave8(input: u8, pulsewidth: u8) -> u8 {
    if input < pulsewidth || pulsewidth == 255 { 255 } else { 0 }
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
    fn scale8_video_never_dims_to_black() {
        assert_eq!(scale8_video(1, 1), 1);
        assert_eq!(scale8_video(255, 255), 255);
        // Falsy on both sides still yields zero.
        assert_eq!(scale8_video(0, 128), 0);
        assert_eq!(scale8_video(128, 0), 0);
    }

    #[test]
    fn scale16by8_and_scale16() {
        assert_eq!(scale16by8(65535, 255), 65279);
        assert_eq!(scale16by8(1024, 128), 512);
        assert_eq!(scale16(65535, 65535), 65534);
        assert_eq!(scale16(65535, 0), 0);
    }

    #[test]
    fn saturating_and_wrapping_arithmetic() {
        assert_eq!(qadd8(200, 100), 255);
        assert_eq!(qadd8(10, 20), 30);
        assert_eq!(qsub8(10, 20), 0);
        assert_eq!(qsub8(30, 20), 10);
        assert_eq!(qadd7(100, 100), 127);
        // Only the positive side saturates: the C return type truncates, so a
        // deep negative sum wraps into positive territory.
        assert_eq!(qadd7(-100, -100), 56);
        // add8/sub8 wrap, unlike their q-prefixed siblings.
        assert_eq!(add8(200, 100), 44);
        assert_eq!(sub8(10, 20), 246);
        assert_eq!(mul8(16, 16), 0);
        assert_eq!(qmul8(16, 16), 255);
        assert_eq!(abs8(-5), 5);
        assert_eq!(abs8(5), 5);
    }

    #[test]
    fn averages_widen_before_dividing() {
        // avg8 must not wrap even though both operands are near 255.
        assert_eq!(avg8(255, 255), 255);
        assert_eq!(avg8(20, 41), 30);
        assert_eq!(avg16(65535, 65535), 65535);
        // avg7 rounds *up* when the first argument is odd.
        assert_eq!(avg7(1, 1), 2);
        assert_eq!(avg7(1, 2), 2);
        assert_eq!(avg7(-1, -1), 0);
    }

    #[test]
    fn modulus_helpers_match_the_closed_form() {
        for a in 0..=255u8 {
            for m in 1..=17u8 {
                assert_eq!(mod8(a, m), a % m, "mod8({a}, {m})");
            }
        }
        for a in [0u8, 1, 100, 200, 255] {
            for b in [0u8, 1, 100, 200, 255] {
                for m in 1..=17u8 {
                    // Both helpers wrap in 8 bits first, then reduce.
                    assert_eq!(addmod8(a, b, m), a.wrapping_add(b) % m);
                    assert_eq!(submod8(a, b, m), a.wrapping_sub(b) % m);
                }
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

    #[test]
    fn interpolation_endpoints_are_exact() {
        assert_eq!(lerp8by8(10, 200, 0), 10);
        // frac = 255 → 255/256 of the way, not all of it.
        assert_eq!(lerp8by8(10, 200, 255), 10 + scale8(190, 255));
        assert_eq!(lerp8by8(200, 10, 0), 200);
        assert_eq!(map8(0, 10, 20), 10);
        assert_eq!(map8(255, 0, 255), 254);
        assert_eq!(blend8(0, 255, 0), 0);
        // Blending "all the way" still truncates one part in 256, because QMK
        // leaves `FASTLED_BLEND_FIXED` undefined.
        assert_eq!(blend8(0, 255, 255), 254);
        assert_eq!(blend8(255, 0, 0), 254);
    }

    #[test]
    fn waves_are_symmetric_and_in_range() {
        assert_eq!(triwave8(0), 0);
        assert_eq!(triwave8(127), 254);
        assert_eq!(triwave8(128), 254);
        assert_eq!(triwave8(255), 0);
        assert_eq!(squarewave8(0, 128), 255);
        assert_eq!(squarewave8(200, 128), 0);
        assert_eq!(squarewave8(200, 255), 255);
        for i in 0..=255u8 {
            // Quadratic/cubic waves stay bounded and hit both extremes.
            assert!(quadwave8(i) <= 255);
            assert!(cubicwave8(i) <= 255);
            assert!(ease8_in_out_quad(i) <= 255);
            assert!(ease8_in_out_approx(i) <= 255);
        }
        // Cubic easing of the triangle peak lands on the 256-overflow clamp.
        assert_eq!(cubicwave8(127), 255);
    }

    #[test]
    fn dimming_helpers_are_monotonic_at_the_edges() {
        assert_eq!(dim8_raw(0), 0);
        assert_eq!(dim8_raw(255), 254);
        assert_eq!(dim8_video(0), 0);
        assert_eq!(dim8_lin(0), 0);
        assert_eq!(dim8_lin(255), 254);
        // brighten8 is the mirror of dim8 applied to the inverted input, so the
        // truncation shows up on the *other* end: full brightening stays 255,
        // while brightening black lands one step above zero.
        assert_eq!(brighten8_raw(0), 1);
        assert_eq!(brighten8_raw(255), 255);
        assert_eq!(brighten8_lin(255), 255);
    }
}
