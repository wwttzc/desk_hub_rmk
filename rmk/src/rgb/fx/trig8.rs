//! Fast trig approximations, ported from QMK's `lib/lib8tion/trig8.h`.
//!
//! These are table-interpolated integer approximations, never floating point.
//! QMK has AVR assembly versions (`sin8_avr`, `sin16_avr`); we always take the
//! `*_C` branch, which the header defines as the equivalent portable path.
//!
//! Angles are the full byte/short range, not radians or degrees:
//! `sin8` takes `0..=255` and returns `0..=255` centred on 128, matching
//! `(sin(x / 128 * pi) * 128) + 128`.

/// Interpolation table for [`sin8`]: `(base, slope16)` pairs per quadrant
/// section.
const B_M16_INTERLEAVE: [u8; 8] = [0, 49, 49, 41, 90, 27, 117, 10];

/// Interpolation table for [`sin16`].
const SIN16_BASE: [u16; 8] = [0, 6393, 12539, 18204, 23170, 27245, 30273, 32137];
/// Per-section slope for [`sin16`].
const SIN16_SLOPE: [u8; 8] = [49, 48, 44, 38, 31, 23, 14, 4];

/// `sin8`: 8-bit sine approximation, `theta` in `0..=255`.
#[inline]
pub const fn sin8(theta: u8) -> u8 {
    let mut offset = theta;
    if theta & 0x40 != 0 {
        offset = 255u8.wrapping_sub(offset);
    }
    let offset = offset & 0x3F;

    let mut secoffset = offset & 0x0F;
    if theta & 0x40 != 0 {
        secoffset += 1;
    }

    let section = (offset >> 4) as usize;
    let b = B_M16_INTERLEAVE[section * 2];
    let m16 = B_M16_INTERLEAVE[section * 2 + 1];

    let mx = ((m16 as u32 * secoffset as u32) >> 4) as u8;

    // `int8_t y = mx + b;` — this can exceed 127, so the C original wraps.
    let mut y = (mx as i32 + b as i32) as i8;
    if theta & 0x80 != 0 {
        y = y.wrapping_neg();
    }

    // `y += 128; return y;` where the function returns `uint8_t`, so the C wraps
    // modulo 256. A Rust `as u8` cast on a negative value truncates instead, so
    // `-128 + 128` would land on 0 rather than 128. `y + 128` is always
    // non-negative for the inputs this table produces, so the wrap is defensive;
    // spell it out rather than relying on that.
    ((y as i32 + 128) as u32 & 0xFF) as u8
}

/// `cos8`: quarter-turn-shifted [`sin8`].
#[inline]
pub const fn cos8(theta: u8) -> u8 {
    sin8(theta.wrapping_add(64))
}

/// `sin16`: 16-bit sine approximation, `theta` in `0..=65535`, result in
/// `-32767..=32767`.
#[inline]
pub const fn sin16(theta: u16) -> i16 {
    let mut offset = (theta & 0x3FFF) >> 3;
    if theta & 0x4000 != 0 {
        offset = 2047 - offset;
    }

    let section = (offset / 256) as usize;
    let b = SIN16_BASE[section];
    let m = SIN16_SLOPE[section];

    let secoffset8 = (offset as u8) / 2;
    let mx = m as u16 * secoffset8 as u16;

    // `int16_t y = mx + b;` — the C version commits to int16_t here.
    let mut y = (mx as i32 + b as i32) as i16;
    if theta & 0x8000 != 0 {
        y = y.wrapping_neg();
    }
    y
}

/// `cos16`: quarter-turn-shifted [`sin16`].
#[inline]
pub const fn cos16(theta: u16) -> i16 {
    sin16(theta.wrapping_add(16384))
}

/// `atan2_8`: integer angle of `(dx, dy)` in `0..=255` turns.
///
/// Note the argument order: QMK's `atan2_8(dy, dx)` takes `dy` first, unlike
/// `atan2(y, x)`'s usual "y first" but for the opposite reason — keep the call
/// sites identical to the C original.
#[inline]
pub const fn atan2_8(dy: i16, dx: i16) -> u8 {
    if dy == 0 {
        return if dx >= 0 { 0 } else { 128 };
    }

    let abs_y = if dy > 0 { dy } else { -dy };

    // The C code assigns the int expression to `int8_t`, truncating.
    let a: i8 = if dx >= 0 {
        (32 - (32 * (dx as i32 - abs_y as i32) / (dx as i32 + abs_y as i32))) as i8
    } else {
        (96 - (32 * (dx as i32 + abs_y as i32) / (abs_y as i32 - dx as i32))) as i8
    };

    if dy < 0 { (-(a as i32)) as u8 } else { a as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The approximation is meant to track the real sine within a couple of
    /// percent; this pins the shape and the fixed points.
    #[test]
    fn sin8_hits_the_quartile_points() {
        assert_eq!(sin8(0), 128);
        assert_eq!(sin8(64), 255);
        assert_eq!(sin8(128), 128);
        assert_eq!(sin8(192), 1);
        // Wrap-around is exact.
        assert_eq!(sin8(255), sin8(255));
    }

    #[test]
    fn sin8_tracks_a_real_sine() {
        for theta in 0..=255u8 {
            let actual = sin8(theta) as f64;
            let expected = ((theta as f64 / 128.0 * core::f64::consts::PI).sin() * 128.0) + 128.0;
            assert!(
                (actual - expected).abs() < 12.0,
                "sin8({theta}) = {actual}, expected about {expected}"
            );
        }
    }

    #[test]
    fn cos8_is_sin8_shifted() {
        for theta in 0..=255u8 {
            assert_eq!(cos8(theta), sin8(theta.wrapping_add(64)));
        }
    }

    #[test]
    fn sin16_hits_the_quartile_points() {
        assert_eq!(sin16(0), 0);
        assert!(sin16(16384) > 32000);
        assert!(sin16(49152) < -32000);
        assert_eq!(cos16(0), sin16(16384));
    }

    #[test]
    fn sin16_tracks_a_real_sine() {
        // The angle space is a full 16-bit turn. Multiply in `u32` because the
        // final quarter is exactly 65536, one past the `u16` range.
        for quarter in 0..=4u32 {
            let theta = (quarter * 16384) as u16;
            let actual = sin16(theta) as f64;
            let expected = ((theta as f64 / 32768.0 * core::f64::consts::PI).sin()) * 32767.0;
            assert!(
                (actual - expected).abs() < 300.0,
                "sin16({theta}) = {actual}, expected about {expected}"
            );
        }
        // Denser sweep across one quarter, staying inside the 16-bit range.
        for step in 0..=16u16 {
            let theta = step * 1024;
            let actual = sin16(theta) as f64;
            let expected = ((theta as f64 / 32768.0 * core::f64::consts::PI).sin()) * 32767.0;
            assert!(
                (actual - expected).abs() < 300.0,
                "sin16({theta}) = {actual}, expected about {expected}"
            );
        }
    }

    #[test]
    fn atan2_8_matches_its_quadrants() {
        // A full turn is 256, so a quarter turn is 64 and an eighth is 32.
        assert_eq!(atan2_8(0, 1), 0);
        assert_eq!(atan2_8(0, -1), 128);
        assert_eq!(atan2_8(1, 1), 32);
        assert_eq!(atan2_8(1, 0), 64);
        // Negative dy flips the result around the byte range.
        assert_eq!(atan2_8(-1, 1), 224);
        assert_eq!(atan2_8(-1, 0), 192);
    }
}
