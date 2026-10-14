//! The effect bodies, ported from QMK's `quantum/rgb_matrix/animations/*.h`.
//!
//! Each function mirrors its C original: same arithmetic, same order of
//! operations, same reliance on byte wrapping. Where the C reaches for a global
//! (`g_led_config`, `rgb_matrix_config`, `g_rgb_timer`, `g_last_hit_tracker`)
//! the value arrives through [`RenderContext`], the [`HitTracker`] argument, or
//! the run-time config.
//!
//! Each effect is `pub` so its tests can call it with known inputs rather than
//! only observing it through the dispatcher.

use crate::rgb::fx::color::{hsv_to_rgb, Hsv, Rgb};
use crate::rgb::fx::math8::{abs8, qadd8, scale8, scale16by8};
use crate::rgb::fx::random8::Rng;
use crate::rgb::fx::trig8::{atan2_8, sin8};
use crate::rgb::led_config::{has_any_flags, has_flags, LED_FLAG_MODIFIER};
use crate::rgb::runner::{EffectParams, LedFrame, RenderContext};
use crate::rgb::state::RgbConfig;

/// `SOLID_COLOR`: the whole chain in the configured colour.
///
/// Not a runner in QMK either — it converts once and fills.
pub fn solid_color<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
) -> bool {
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let rgb = hsv_to_rgb(ctx.config.hsv);

    for i in led_min..led_max {
        if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
            continue;
        }
        frame.set_color(i as usize, rgb);
    }

    ctx.more_batches(led_max)
}

/// `ALPHAS_MODS`: alphas in the configured colour, modifiers in the colour
/// `speed` hue-steps away.
///
/// The second colour is derived by adding `speed` to the hue and relies on the
/// byte wrap.
pub fn alphas_mods<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
) -> bool {
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);

    let hsv = ctx.config.hsv;
    let rgb_alpha = hsv_to_rgb(hsv);
    let rgb_mod = hsv_to_rgb(hsv.with_h(hsv.h.wrapping_add(ctx.config.speed)));

    for i in led_min..led_max {
        let flags = ctx.leds.flags(i as usize);
        if !has_any_flags(flags, params.flags) {
            continue;
        }
        let rgb = if has_flags(flags, LED_FLAG_MODIFIER) {
            rgb_mod
        } else {
            rgb_alpha
        };
        frame.set_color(i as usize, rgb);
    }

    ctx.more_batches(led_max)
}

/// `GRADIENT_UP_DOWN`: a static top-to-bottom hue ramp.
///
/// `speed` controls how far the ramp departs from the base hue. The y range
/// `0..=64` maps to `0..=4` steps of `scale`, and the hue wrap is deliberate.
pub fn gradient_up_down<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
) -> bool {
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let scale = scale8(64, ctx.config.speed);
    let base = ctx.config.hsv;

    for i in led_min..led_max {
        if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
            continue;
        }
        let y = ctx.leds.point(i as usize).y;
        let hsv = base.with_h(base.h.wrapping_add(scale.wrapping_mul(y >> 4)));
        frame.set_color(i as usize, hsv_to_rgb(hsv));
    }

    ctx.more_batches(led_max)
}

/// `GRADIENT_LEFT_RIGHT`: a static left-to-right hue ramp.
///
/// Unlike the up/down version the x range `0..=224` maps to `0..=7` steps, so
/// the shift is `scale * x >> 5`.
pub fn gradient_left_right<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
) -> bool {
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let scale = scale8(64, ctx.config.speed);
    let base = ctx.config.hsv;

    for i in led_min..led_max {
        if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
            continue;
        }
        let x = ctx.leds.point(i as usize).x;
        // The C is `scale * x >> 5` in int, so widen before shifting.
        let shift = ((scale as u16 * x as u16) >> 5) as u8;
        let hsv = base.with_h(base.h.wrapping_add(shift));
        frame.set_color(i as usize, hsv_to_rgb(hsv));
    }

    ctx.more_batches(led_max)
}

/// The breath term shared by the brightness- and hue-breathing effects:
/// `abs8(sin8(t) - 128) * 2`.
///
/// The multiply happens in `int` in the C, because `abs8` returns `int8_t` and
/// integer promotion widens it before the `* 2`. That matters at the one input
/// where `abs8` cannot be represented positively: `abs8(-128)` is `-128`, so
/// `-128 * 2 == -256`, and the conversion into `scale8`'s `uint8_t` parameter
/// truncates it to 0. Computing this in `i32` and then narrowing reproduces
/// that; doing it in `u16` would overflow instead.
#[inline]
fn abs8_times2(wave: i8) -> u8 {
    (abs8(wave) as i32 * 2) as u8
}

/// `abs8(sin8(t) - 128) * 2`, the shape both breathing effects use.
#[inline]
fn breath_term(time: u8) -> u8 {
    abs8_times2((sin8(time) as i16 - 128) as i8)
}

/// `BREATHING_math`: whole-chain brightness fade.
pub fn breathing_math(hsv: Hsv, _i: u8, time: u8) -> Hsv {
    hsv.with_v(scale8(breath_term(time / 2), hsv.v))
}

/// `CYCLE_ALL_math`: hue sweeps the full circle in lockstep.
pub fn cycle_all_math(hsv: Hsv, _i: u8, time: u8) -> Hsv {
    hsv.with_h(time)
}

/// `CYCLE_LEFT_RIGHT_math`: hue offset by each LED's x position.
pub fn cycle_left_right_math(hsv: Hsv, x: u8, time: u8) -> Hsv {
    hsv.with_h(x.wrapping_sub(time))
}

/// `CYCLE_UP_DOWN_math`: hue offset by each LED's y position.
pub fn cycle_up_down_math(hsv: Hsv, y: u8, time: u8) -> Hsv {
    hsv.with_h(y.wrapping_sub(time))
}

/// The band offset shared by `BAND_SAT` and `BAND_VAL`:
/// `abs(scale8(x, 228) + 28 - time)`, computed in `int` as the C does.
#[inline]
fn band_offset(x: u8, time: u8) -> u8 {
    // Clippy suggests `unsigned_abs`, which for `i16::MIN` would yield 32768 and
    // then truncate to 0 — the same byte result here, but only by accident. The
    // signed `abs` states the C's intent (a magnitude in 0..=32767), so keep it.
    #[allow(clippy::cast_abs_to_unsigned)]
    let offset = (scale8(x, 228) as i16 + 28 - time as i16).abs() as u8;
    offset
}

/// `BAND_SAT_math`: a saturation band sweeping left to right.
///
/// The `int16_t s` and the `s < 0` clamp are load-bearing: the subtraction goes
/// negative over most of the sweep, and `scale8` would otherwise wrap a
/// negative into a large positive.
pub fn band_sat_math(hsv: Hsv, x: u8, time: u8) -> Hsv {
    let s = hsv.s as i16 - band_offset(x, time) as i16 * 8;
    let s = if s < 0 { 0 } else { s as u8 };
    Hsv {
        s: scale8(s, hsv.s),
        ..hsv
    }
}

/// `BAND_VAL_math`: a brightness band sweeping left to right.
pub fn band_val_math(hsv: Hsv, x: u8, time: u8) -> Hsv {
    let v = hsv.v as i16 - band_offset(x, time) as i16 * 8;
    let v = if v < 0 { 0 } else { v as u8 };
    Hsv {
        v: scale8(v, hsv.v),
        ..hsv
    }
}

/// `BAND_PINWHEEL_SAT_math`: the saturation band spun around the centre.
pub fn band_pinwheel_sat_math(hsv: Hsv, dx: i16, dy: i16, time: u8) -> Hsv {
    let s = hsv
        .s
        .wrapping_sub(time)
        .wrapping_sub(atan2_8(dy, dx).wrapping_mul(3));
    Hsv {
        s: scale8(s, hsv.s),
        ..hsv
    }
}

/// `BAND_PINWHEEL_VAL_math`.
pub fn band_pinwheel_val_math(hsv: Hsv, dx: i16, dy: i16, time: u8) -> Hsv {
    let v = hsv
        .v
        .wrapping_sub(time)
        .wrapping_sub(atan2_8(dy, dx).wrapping_mul(3));
    Hsv {
        v: scale8(v, hsv.v),
        ..hsv
    }
}

/// `BAND_SPIRAL_SAT_math`: the band spiralled outwards.
///
/// This one *adds* `dist` before subtracting the angle, unlike the pinwheel
/// above — the signs are QMK's.
pub fn band_spiral_sat_math(hsv: Hsv, dx: i16, dy: i16, dist: u8, time: u8) -> Hsv {
    let s = hsv
        .s
        .wrapping_add(dist)
        .wrapping_sub(time)
        .wrapping_sub(atan2_8(dy, dx));
    Hsv {
        s: scale8(s, hsv.s),
        ..hsv
    }
}

/// `BAND_SPIRAL_VAL_math`.
pub fn band_spiral_val_math(hsv: Hsv, dx: i16, dy: i16, dist: u8, time: u8) -> Hsv {
    let v = hsv
        .v
        .wrapping_add(dist)
        .wrapping_sub(time)
        .wrapping_sub(atan2_8(dy, dx));
    Hsv {
        v: scale8(v, hsv.v),
        ..hsv
    }
}

/// `CYCLE_PINWHEEL_math`: hue gradient spun around the centre.
pub fn cycle_pinwheel_math(hsv: Hsv, dx: i16, dy: i16, time: u8) -> Hsv {
    hsv.with_h(atan2_8(dy, dx).wrapping_add(time))
}

/// `CYCLE_SPIRAL_math`: hue gradient spiralled outwards.
pub fn cycle_spiral_math(hsv: Hsv, dx: i16, dy: i16, dist: u8, time: u8) -> Hsv {
    hsv.with_h(dist.wrapping_sub(time).wrapping_sub(atan2_8(dy, dx)))
}

/// `CYCLE_OUT_IN_math`: concentric rings of hue.
///
/// `3 * dist / 2` is `int` arithmetic in the C, so widen before scaling.
pub fn cycle_out_in_math(hsv: Hsv, dist: u8, time: u8) -> Hsv {
    let shift = (3 * dist as u16 / 2) as u8;
    hsv.with_h(shift.wrapping_add(time))
}

/// `HUE_BREATHING_math`: the hue oscillates within a small window.
///
/// `delta = 12` is hard-coded in QMK, not derived from `speed`.
pub fn hue_breathing_math(hsv: Hsv, _i: u8, time: u8) -> Hsv {
    const DELTA: u8 = 12;
    hsv.with_h(hsv.h.wrapping_add(scale8(breath_term(time / 2), DELTA)))
}

/// `HUE_PENDULUM_math`: the hue window sweeps right then reverses.
///
/// `sin8(time) + x` is computed in `int` and then narrowed by the `- 128`, so
/// it wraps at the byte boundary rather than saturating.
pub fn hue_pendulum_math(hsv: Hsv, x: u8, time: u8) -> Hsv {
    const HUEDELTA: u8 = 12;
    let wave = (sin8(time) as i16 + x as i16 - 128) as i8;
    hsv.with_h(hsv.h.wrapping_add(scale8(abs8_times2(wave), HUEDELTA)))
}

/// `HUE_WAVE_math`: a rightward hue wave.
pub fn hue_wave_math(hsv: Hsv, x: u8, time: u8) -> Hsv {
    const HUEDELTA: u8 = 24;
    // The C passes the `int8_t` from `abs8` straight into `scale8`'s `uint8_t`
    // parameter, so the non-negative result converts without a cast.
    let wave = abs8(x.wrapping_sub(time) as i8) as u8;
    hsv.with_h(hsv.h.wrapping_add(scale8(wave, HUEDELTA)))
}

/// `RAINBOW_BEACON_math`: a hue gradient rotating around the centre.
///
/// The C computes this in `int` and assigns back to a `u8`, so the intermediate
/// is signed and the truncation applies to the whole expression. `cos` is the
/// runner's **first** positional argument — QMK's parameter is named `sin`,
/// which does not match its call sites.
pub fn rainbow_beacon_math(hsv: Hsv, cos: i8, sin: i8, x: u8, y: u8, center_x: u8, center_y: u8) -> Hsv {
    let term_y = (y as i16 - center_y as i16) * 2 * cos as i16;
    let term_x = (x as i16 - center_x as i16) * 2 * sin as i16;
    let delta = ((term_y + term_x) / 128) as i8;
    hsv.with_h(hsv.h.wrapping_add(delta as u8))
}

/// `RIVERFLOW_math`: a per-LED phase offset on the breathing curve.
///
/// `i * 315` is computed in `uint32_t` in the C, so widen here too. Note the
/// runner already computed `time` from the timer; the effect recomputes its own
/// from the raw timer, which is why both are parameters.
pub fn riverflow_math(hsv: Hsv, i: u8, timer: u32, speed: u8) -> Hsv {
    let phase = scale16by8((timer + i as u32 * 315) as u16, speed / 8);
    hsv.with_v(scale8(breath_term(phase as u8), hsv.v))
}
/// `SOLID_REACTIVE_SIMPLE_math`: a brightness pulse that decays with the hit's
/// age.
///
/// `offset` is clamped to 255 first, because the caller can hand over
/// `max_tick` (up to 65535) when the LED was never hit.
pub fn solid_reactive_simple_math(hsv: Hsv, offset: u16) -> Hsv {
    let offset = if offset > 255 { 255 } else { offset } as u8;
    hsv.with_v(scale8(255u8.wrapping_sub(offset), hsv.v))
}

/// `SOLID_REACTIVE_math`: like the simple variant, but the pulse also shifts
/// the hue by up to 64 steps.
pub fn solid_reactive_math(hsv: Hsv, offset: u16) -> Hsv {
    let offset = if offset > 255 { 255 } else { offset } as u8;
    hsv.with_h(hsv.h.wrapping_add(scale8(255u8.wrapping_sub(offset), 64)))
}

/// `RAINDROPS`' per-LED colour: the base hue nudged by a small random step.
///
/// QMK's `deltaH` looks like it wants the distance to the opposite hue, but the
/// expression `(int8_t)((hsv.h + 128) - hsv.h) / 4` cancels the `hsv.h` terms,
/// leaving `(int8_t)128 / 4`. Casting 128 to `int8_t` is itself an overflowing
/// conversion — `128u8 as i8` is `-128` — so the result is a constant **-32**.
/// Reproduced as written; a "tidied" `128 / 4 == 32` would change every hue the
/// effect produces.
#[inline]
pub fn raindrops_delta_h() -> i8 {
    // `128u8 as i8` is the cast the C performs; spell it out so the wrap is
    // visible rather than hidden behind an out-of-range literal.
    let h: i8 = 128u8 as i8;
    h / 4
}

/// `RAINDROPS`' colour for one LED.
pub fn raindrops_color(config: &RgbConfig, rng: &mut Rng) -> Rgb {
    let mut hsv = config.hsv;
    let jitter = raindrops_delta_h() as i16 * rng.next8_max(3) as i16;
    hsv.h = hsv.h.wrapping_add(jitter as u8);
    hsv_to_rgb(hsv)
}

/// `RAINDROPS`: re-colour one LED at a time on a periodic trigger.
///
/// The trigger tests `params.iter == 0`, so it fires once per frame rather than
/// once per batch. `index` is QMK's `static uint16_t index`: it selects the LED
/// to repaint and is parked out of range until the next trigger.
pub fn raindrops<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    rng: &mut Rng,
    index: &mut u16,
    frame: &mut F,
) -> bool {
    // Periodic trigger for LED change. The modulo is written as the C has it;
    // `is_multiple_of` reads the same but hides that the original is a remainder
    // test on a value the speed can zero out.
    #[allow(clippy::manual_is_multiple_of)]
    if params.iter == 0 && scale16by8(ctx.time as u16, qadd8(ctx.config.speed, 16)) % 10 == 0 {
        *index = rng.next8_max(LED_COUNT as u8) as u16;
    }

    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    if params.init {
        for i in led_min..led_max {
            if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
                continue;
            }
            frame.set_color(i as usize, raindrops_color(ctx.config, rng));
        }
    } else if led_min as u16 <= *index && *index < led_max as u16 {
        if has_any_flags(ctx.leds.flags(*index as usize), params.flags) {
            frame.set_color(*index as usize, raindrops_color(ctx.config, rng));
        }
        // Park the index until the next trigger.
        *index = u16::MAX;
    }

    ctx.more_batches(led_max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::led_config::{LedConfig, LedPoint, LED_FLAG_ALL, LED_FLAG_KEYLIGHT, LED_FLAG_MODIFIER};
    use crate::rgb::runner::ArrayFrame;

    const N: usize = 4;

    /// Four LEDs on a 2x2 grid: (0,0) (0,1) top, (1,0) (1,1) bottom, with
    /// x spread across the coordinate space so positional effects differ.
    fn leds() -> LedConfig<N> {
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

    fn config() -> RgbConfig {
        RgbConfig::with_defaults(true, 100, 200, 128, LED_FLAG_ALL, 255)
    }

    /// One `process_limit` that disables batching, so each call is a whole frame.
    fn ctx<'a>(leds: &'a LedConfig<N>, config: &'a RgbConfig) -> RenderContext<'a, N> {
        RenderContext {
            leds,
            config,
            center: RenderContext::<N>::DEFAULT_CENTER,
            time: 1000,
            process_limit: N,
        }
    }

    fn params() -> EffectParams {
        EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        }
    }

    #[test]
    fn solid_color_fills_every_led_with_the_configured_colour() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut frame = ArrayFrame::<N>::new();

        solid_color(&ctx, &params(), &mut frame);
        assert_eq!(frame.pixels, [hsv_to_rgb(config.hsv); N]);
    }

    #[test]
    fn alphas_mods_splits_modifiers_from_alphas() {
        // LED 1 is a modifier, the rest are keylights.
        let leds = LedConfig::new(
            [LedPoint::new(0, 0); N],
            [
                LED_FLAG_KEYLIGHT,
                LED_FLAG_MODIFIER,
                LED_FLAG_KEYLIGHT,
                LED_FLAG_KEYLIGHT,
            ],
            [(0, 0), (0, 1), (1, 0), (1, 1)],
        );
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut frame = ArrayFrame::<N>::new();

        alphas_mods(&ctx, &params(), &mut frame);

        let alpha = hsv_to_rgb(config.hsv);
        let modifier = hsv_to_rgb(config.hsv.with_h(config.hsv.h.wrapping_add(config.speed)));
        assert_ne!(alpha, modifier, "the two colours must differ for this test to mean anything");
        assert_eq!(frame.pixels[0], alpha);
        assert_eq!(frame.pixels[1], modifier);
        assert_eq!(frame.pixels[2], alpha);
    }

    #[test]
    fn gradient_up_down_offsets_hue_by_row() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut frame = ArrayFrame::<N>::new();

        gradient_up_down(&ctx, &params(), &mut frame);

        let scale = scale8(64, config.speed);
        // LEDs 0/1 sit at y = 0 and keep the base hue; 2/3 at y = 64 shift by
        // `scale * (64 >> 4)` = `scale * 4`.
        assert_eq!(frame.pixels[0], hsv_to_rgb(config.hsv));
        assert_eq!(frame.pixels[1], frame.pixels[0], "same row, same colour");
        let expected = hsv_to_rgb(config.hsv.with_h(config.hsv.h.wrapping_add(scale.wrapping_mul(4))));
        assert_eq!(frame.pixels[2], expected);
        assert_eq!(frame.pixels[3], expected, "same row, same colour");
        assert_ne!(frame.pixels[1], frame.pixels[2], "the ramp is visible between rows");
    }

    #[test]
    fn gradient_left_right_offsets_hue_by_column() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut frame = ArrayFrame::<N>::new();

        gradient_left_right(&ctx, &params(), &mut frame);

        let scale = scale8(64, config.speed);
        for i in 0..N {
            let x = leds.point(i).x;
            let shift = (scale as u16 * x as u16 >> 5) as u8;
            assert_eq!(
                frame.pixels[i],
                hsv_to_rgb(config.hsv.with_h(config.hsv.h.wrapping_add(shift))),
                "LED {i} at x = {x}"
            );
        }
        assert_ne!(frame.pixels[0], frame.pixels[N - 1], "the ramp is visible across the row");
    }

    #[test]
    fn breathing_scales_brightness_and_leaves_colour_alone() {
        let hsv = Hsv::new(50, 200, 255);
        // `sin8(0) == 128`, so the breath term is zero and the LED is dark.
        assert_eq!(breathing_math(hsv, 0, 0).v, 0);
        // The sine peak is a quarter turn in: `sin8(64) == 255`, which makes the
        // term `|255 - 128| * 2 == 254`, so brightness loses one part in 256.
        let peak = breathing_math(hsv, 0, 128);
        assert_eq!(peak.v, scale8(254, 255));
        assert_eq!(peak.h, hsv.h, "hue is untouched");
        assert_eq!(peak.s, hsv.s, "saturation is untouched");
    }

    #[test]
    fn cycle_all_sets_hue_to_time_and_nothing_else() {
        let hsv = Hsv::new(10, 20, 30);
        let out = cycle_all_math(hsv, 0, 77);
        assert_eq!(out.h, 77);
        assert_eq!((out.s, out.v), (20, 30), "only hue moves");
    }

    #[test]
    fn cycle_left_right_and_up_down_use_position_and_wrap() {
        let hsv = Hsv::new(10, 200, 200);
        assert_eq!(cycle_left_right_math(hsv, 100, 30).h, 70);
        assert_eq!(cycle_up_down_math(hsv, 100, 30).h, 70);
        // The subtraction wraps at the byte boundary rather than saturating.
        assert_eq!(cycle_left_right_math(hsv, 10, 200).h, 66);
    }

    #[test]
    fn band_sat_clamps_the_negative_intermediate() {
        let hsv = Hsv::new(0, 255, 255);
        // `time == 28` with `x == 0` puts the band at zero offset, so
        // saturation is only touched by the final `scale8`.
        let centred = band_sat_math(hsv, 0, 28);
        assert_eq!(centred.s, scale8(255, 255));
        assert_eq!(centred.v, hsv.v, "brightness is untouched");
        // Away from the band the subtraction goes negative, and the clamp keeps
        // it at zero instead of letting it wrap to a bright value.
        let far = band_sat_math(hsv, 224, 28);
        assert!(far.s < centred.s, "saturation drops outside the band: {}", far.s);
    }

    #[test]
    fn band_val_clamps_the_negative_intermediate() {
        let hsv = Hsv::new(0, 255, 255);
        let centred = band_val_math(hsv, 0, 28);
        assert_eq!(centred.v, scale8(255, 255));
        assert_eq!(centred.s, hsv.s, "saturation is untouched");
        let far = band_val_math(hsv, 224, 28);
        assert!(far.v < centred.v, "brightness drops outside the band: {}", far.v);
    }

    #[test]
    fn band_pinwheel_lowers_saturation_with_a_nonzero_angle() {
        let hsv = Hsv::new(0, 255, 255);
        // `atan2_8(dy, dx)` is 0 when dy == 0 and dx > 0.
        let no_angle = band_pinwheel_sat_math(hsv, 1, 0, 0);
        assert_eq!(no_angle.s, scale8(255, 255));
        // A quarter turn subtracts `64 * 3` before scaling.
        let quarter = band_pinwheel_sat_math(hsv, 0, 1, 0);
        assert_ne!(quarter.s, no_angle.s, "the angle term changes saturation");
    }

    #[test]
    fn spiral_effects_add_distance_before_subtracting_time() {
        let hsv = Hsv::new(0, 255, 255);
        let near = band_spiral_sat_math(hsv, 1, 0, 0, 0);
        assert_eq!(near.s, scale8(255, 255), "no distance, no time: unchanged");
        // A larger distance feeds a larger value into `scale8`.
        let far = band_spiral_sat_math(hsv, 1, 0, 50, 0);
        assert_ne!(far.s, near.s);
        // The val variant mirrors it on brightness.
        assert_eq!(band_spiral_val_math(hsv, 1, 0, 0, 0).v, scale8(255, 255));
        assert_ne!(band_spiral_val_math(hsv, 1, 0, 50, 0).v, scale8(255, 255));
    }

    #[test]
    fn cycle_pinwheel_offsets_hue_by_angle_and_time() {
        let hsv = Hsv::new(0, 255, 255);
        // dy == 0, dx > 0 gives angle 0, so the hue is just `time`.
        assert_eq!(cycle_pinwheel_math(hsv, 1, 0, 42).h, 42);
        // dy > 0, dx == 0 is a quarter turn: `atan2_8(1, 0) == 64`.
        assert_eq!(cycle_pinwheel_math(hsv, 0, 1, 0).h, 64);
        assert_eq!(cycle_pinwheel_math(hsv, 0, 1, 10).h, 74, "time adds on top");
    }

    #[test]
    fn cycle_spiral_subtracts_both_angle_and_time() {
        let hsv = Hsv::new(0, 255, 255);
        // No distance, no angle, no time: unchanged.
        assert_eq!(cycle_spiral_math(hsv, 1, 0, 0, 0).h, 0);
        // `dist - time - angle`, all wrapping.
        assert_eq!(cycle_spiral_math(hsv, 1, 0, 10, 3).h, 7);
        // The angle here is 64, so 0 - 0 - 64 wraps to 192.
        assert_eq!(cycle_spiral_math(hsv, 0, 1, 0, 0).h, 192);
    }

    #[test]
    fn cycle_out_in_scales_distance_by_three_halves() {
        let hsv = Hsv::new(0, 255, 255);
        assert_eq!(cycle_out_in_math(hsv, 0, 0).h, 0);
        assert_eq!(cycle_out_in_math(hsv, 2, 0).h, 3, "3 * 2 / 2 == 3");
        assert_eq!(cycle_out_in_math(hsv, 10, 0).h, 15);
        assert_eq!(cycle_out_in_math(hsv, 10, 5).h, 20, "time adds on top");
    }

    #[test]
    fn hue_breathing_uses_its_hard_coded_delta() {
        let hsv = Hsv::new(100, 200, 200);
        // At time zero the breath term is zero.
        assert_eq!(hue_breathing_math(hsv, 0, 0).h, 100);
        // At the peak it adds `scale8(255, 12)`.
        let peak = hue_breathing_math(hsv, 0, 128);
        assert_eq!(peak.h, 100u8.wrapping_add(scale8(255, 12)));
        assert_eq!((peak.s, peak.v), (200, 200), "saturation and value are untouched");
    }

    #[test]
    fn hue_pendulum_shifts_differently_per_position() {
        let hsv = Hsv::new(0, 255, 255);
        // With `time == 0`, `sin8(0) == 128`, so the wave is just the position.
        let origin = hue_pendulum_math(hsv, 0, 0);
        let right = hue_pendulum_math(hsv, 64, 0);
        assert_ne!(origin.h, right.h, "position changes the shift");
        assert_eq!(origin.h, 0, "a zero wave leaves the hue alone");
        assert_eq!(right.h, scale8(128, 12));
    }

    /// One position produces no shift at all, because the wave widens into the
    /// signed range: `sin8(0) + 128 - 128 == 128`, which as an `int8_t` is
    /// `-128`, and `abs8(-128) * 2` then truncates to `0`. A "fixed" `abs`
    /// would light that column instead.
    #[test]
    fn hue_pendulum_wraps_at_the_signed_boundary() {
        let hsv = Hsv::new(0, 255, 255);
        assert_eq!(hue_pendulum_math(hsv, 128, 0).h, 0, "the boundary column wraps to no shift");
    }

    #[test]
    fn hue_wave_depends_on_position_minus_time() {
        let hsv = Hsv::new(0, 255, 255);
        // `x == time` means no shift at all.
        assert_eq!(hue_wave_math(hsv, 50, 50).h, 0);
        // A larger gap gives a larger shift, bounded by the delta.
        let near = hue_wave_math(hsv, 51, 50).h;
        let far = hue_wave_math(hsv, 100, 50).h;
        assert!(near < far, "the wave grows with distance: {near} vs {far}");
        assert_eq!(far, scale8(50, 24));
    }

    #[test]
    fn rainbow_beacon_leaves_a_centred_led_alone() {
        let hsv = Hsv::new(0, 255, 255);
        let center = RenderContext::<N>::DEFAULT_CENTER;
        // An LED exactly at the centre has zero offset, so zero hue shift.
        let centred = rainbow_beacon_math(hsv, 0, 0, center.x, center.y, center.x, center.y);
        assert_eq!(centred.h, 0);
        // An off-centre LED with a non-zero cosine or sine does shift.
        let off = rainbow_beacon_math(hsv, 100, 0, 224, 0, center.x, center.y);
        assert_ne!(off.h, 0);
    }

    #[test]
    fn riverflow_gives_adjacent_leds_distinct_phases() {
        let hsv = Hsv::new(0, 255, 255);
        let a = riverflow_math(hsv, 0, 1000, 128);
        let b = riverflow_math(hsv, 1, 1000, 128);
        assert_ne!(a.v, b.v, "adjacent LEDs are out of phase");
        // With speed < 8 the phase scale is zero, so every LED agrees.
        let all_same_a = riverflow_math(hsv, 0, 1000, 0);
        let all_same_b = riverflow_math(hsv, 1, 1000, 0);
        assert_eq!(all_same_a.v, all_same_b.v);
    }

    #[test]
    fn reactive_simple_clamps_the_offset_before_decaying() {
        let hsv = Hsv::new(0, 255, 255);
        // A fresh hit (offset 0) is at full configured brightness.
        assert_eq!(solid_reactive_simple_math(hsv, 0).v, scale8(255, 255));
        // A saturated offset fully suppresses the LED.
        assert_eq!(solid_reactive_simple_math(hsv, 255).v, 0);
        // Beyond 255 the value is clamped, not wrapped.
        assert_eq!(solid_reactive_simple_math(hsv, 300).v, 0);
        assert_eq!(solid_reactive_simple_math(hsv, 65535).v, 0);
    }

    #[test]
    fn reactive_shifts_hue_as_well_as_brightness() {
        let hsv = Hsv::new(0, 255, 255);
        assert_eq!(solid_reactive_math(hsv, 0).h, scale8(255, 64));
        assert_eq!(solid_reactive_math(hsv, 255).h, 0);
    }

    /// `deltaH` in QMK cancels to a constant. Pinned so that if upstream ever
    /// fixes the expression, the difference shows up here rather than as a
    /// mysterious visual change on the keyboard.
    #[test]
    fn raindrops_delta_h_is_the_constant_qmk_produces() {
        assert_eq!(raindrops_delta_h(), -32);
    }

    #[test]
    fn raindrops_init_fills_the_chain_then_repaints_one() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut rng = Rng::with_seed(1337);
        let mut index = u16::MAX;
        let mut frame = ArrayFrame::<N>::new();
        frame.fill(Rgb::BLACK);

        // A fresh frame paints every LED.
        raindrops(&ctx, &params(), &mut rng, &mut index, &mut frame);
        assert!(
            frame.pixels.iter().all(|p| *p != Rgb::BLACK),
            "init fills the whole chain"
        );

        // A later batch repaints at most one LED, then parks the index.
        frame.fill(Rgb::BLACK);
        let later = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: false,
        };
        raindrops(&ctx, &later, &mut rng, &mut index, &mut frame);
        let painted = frame.pixels.iter().filter(|p| **p != Rgb::BLACK).count();
        assert!(painted <= 1, "at most one LED changes per batch, got {painted}");
    }

    /// The trigger tests `iter == 0`, so a multi-batch frame does not re-roll
    /// the target LED once per batch.
    #[test]
    fn raindrops_trigger_only_fires_on_the_first_batch() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut rng = Rng::with_seed(1337);
        let mut index = u16::MAX;
        let mut frame = ArrayFrame::<N>::new();
        frame.fill(Rgb::BLACK);

        let non_first = EffectParams {
            iter: 1,
            flags: LED_FLAG_ALL,
            init: false,
        };
        raindrops(&ctx, &non_first, &mut rng, &mut index, &mut frame);
        assert_eq!(index, u16::MAX, "a non-first batch never rolls a new index");
        assert_eq!(frame.pixels, [Rgb::BLACK; N], "and paints nothing");
    }

    #[test]
    fn raindrops_skips_leds_the_flag_filter_excludes() {
        let leds = LedConfig::new(
            [LedPoint::new(0, 0); N],
            [LED_FLAG_KEYLIGHT; N],
            [(0, 0), (0, 1), (1, 0), (1, 1)],
        );
        let config = config();
        let ctx = ctx(&leds, &config);
        let mut rng = Rng::with_seed(1337);
        let mut index = u16::MAX;
        let mut frame = ArrayFrame::<N>::new();

        // Nothing carries the modifier flag, so nothing is drawn.
        let only_mods = EffectParams {
            iter: 0,
            flags: LED_FLAG_MODIFIER,
            init: true,
        };
        raindrops(&ctx, &only_mods, &mut rng, &mut index, &mut frame);
        assert_eq!(frame.pixels, [Rgb::BLACK; N]);
    }
}
