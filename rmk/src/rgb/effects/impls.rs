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
use crate::rgb::fx::math8::{abs8, qadd8, qsub8, scale8, scale16by8, sqrt16};
use crate::rgb::fx::random8::Rng;
use crate::rgb::fx::trig8::{atan2_8, sin8};
use crate::rgb::led_config::{has_any_flags, has_flags, LedInfo, LedPoint, LED_FLAG_MODIFIER};
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

/// `DUAL_BEACON_math`: two hue gradients sweeping in from opposite corners.
///
/// The same shape as [`rainbow_beacon_math`] but **without** the `* 2` factor on
/// the two position terms, so the pattern is a half-turn wider.
///
/// The parameters are named as QMK's effect declares them, `(sin, cos)`, even
/// though the runner's first argument is the cosine — see
/// [`effect_runner_sin_cos_i`](crate::rgb::runner::effect_runner_sin_cos_i).
pub fn dual_beacon_math(hsv: Hsv, cos: i8, sin: i8, x: u8, y: u8, center_x: u8, center_y: u8) -> Hsv {
    let term_y = (y as i16 - center_y as i16) * cos as i16;
    let term_x = (x as i16 - center_x as i16) * sin as i16;
    let delta = ((term_y + term_x) / 128) as i8;
    hsv.with_h(hsv.h.wrapping_add(delta as u8))
}

/// `RAINBOW_BEACON_math`: a hue gradient rotating around the centre.
///
/// The C computes this in `int` and assigns back to a `u8`, so the intermediate
/// is signed and the truncation applies to the whole expression.
pub fn rainbow_beacon_math(hsv: Hsv, cos: i8, sin: i8, x: u8, y: u8, center_x: u8, center_y: u8) -> Hsv {
    let term_y = (y as i16 - center_y as i16) * 2 * cos as i16;
    let term_x = (x as i16 - center_x as i16) * 2 * sin as i16;
    let delta = ((term_y + term_x) / 128) as i8;
    hsv.with_h(hsv.h.wrapping_add(delta as u8))
}

/// `RAINBOW_PINWHEELS_math`: two hue gradients spinning opposite halves.
///
/// The x term folds about the centre (`56 - abs8(dx)`), which is what makes the
/// two halves counter-rotate; `56` is 224/4, hard-coded in QMK.
pub fn rainbow_pinwheels_math(hsv: Hsv, cos: i8, sin: i8, x: u8, y: u8, center_x: u8, center_y: u8) -> Hsv {
    let term_y = (y as i16 - center_y as i16) * 3 * cos as i16;
    // The C subtracts two `uint8_t` and passes the wrapped result to `abs8`'s
    // `int8_t` parameter, so the subtraction has to wrap rather than overflow.
    let folded_x = 56i16 - abs8(x.wrapping_sub(center_x) as i8) as i16;
    let term_x = folded_x * 3 * sin as i16;
    let delta = ((term_y + term_x) / 128) as u8;
    hsv.with_h(hsv.h.wrapping_add(delta))
}

/// `CYCLE_OUT_IN_DUAL_math`: concentric rings radiating from two foci.
///
/// `dx` is folded about the centre first, so the rings grow out of the middle of
/// each half instead of out of one point. `abs8(dy)` narrows the signed offset
/// to a byte, which is what QMK's `uint8_t dist` intermediate does.
pub fn cycle_out_in_dual_math(hsv: Hsv, dx: i16, dy: i16, time: u8, center_x: u8) -> Hsv {
    let folded = (center_x as i16 / 2) - abs8(dx as i8) as i16;
    let dist = sqrt16((folded * folded) as u16 + (abs8(dy as i8) as u16) * (abs8(dy as i8) as u16));
    // `3 * dist + time` is an `int` in the C and truncates on assignment to the
    // byte-wide hue, so widen only for the multiply and wrap on the add.
    let shift = (3 * dist as u16) as u8;
    hsv.with_h(shift.wrapping_add(time))
}

/// `RAINBOW_MOVING_CHEVRON_math`: a V-shaped hue front sweeping rightwards.
///
/// The y term is what bends it into a chevron; without it the effect would be
/// `CYCLE_LEFT_RIGHT`.
pub fn rainbow_moving_chevron_math(
    hsv: Hsv,
    x: u8,
    y: u8,
    time: u8,
    center_y: u8,
) -> Hsv {
    let chevron = abs8(y as i8 - center_y as i8) as u8;
    hsv.with_h(hsv.h.wrapping_add(chevron).wrapping_add(x.wrapping_sub(time)))
}

/// `SOLID_REACTIVE_WIDE_math`: like [`splash_math`] over a ring that fades fast,
/// because the distance is scaled by 5 before it is added to the age.
pub fn solid_reactive_wide_math(hsv: Hsv, _dx: i16, _dy: i16, dist: u8, tick: u16) -> Hsv {
    let effect = (tick + dist as u16 * 5).min(255) as u8;
    hsv.with_v(qadd8(hsv.v, 255 - effect))
}

/// `SOLID_REACTIVE_CROSS_math`: a plus-shaped pulse along the hit's row and
/// column.
///
/// The smaller of the two scaled offsets wins, so the light travels faster along
/// the axis the LED is further from — which is what draws the cross rather than
/// a square. `dist` itself only enters through the age term.
pub fn solid_reactive_cross_math(hsv: Hsv, dx: i16, dy: i16, dist: u8, tick: u16) -> Hsv {
    let mut effect = tick.wrapping_add(dist as u16);
    let spread_x = (abs_i16(dx) as u16 * 16).min(255);
    let spread_y = (abs_i16(dy) as u16 * 16).min(255);
    effect = effect.saturating_add(spread_x.min(spread_y));
    let effect = effect.min(255) as u8;
    hsv.with_v(qadd8(hsv.v, 255 - effect))
}

/// `abs` on a signed LED offset.
///
/// The C writes `dx < 0 ? dx * -1 : dx` on an `int16_t`, which is the same
/// thing; a named helper keeps the three reactive variants reading alike.
#[inline]
fn abs_i16(value: i16) -> i16 {
    value.abs()
}

/// `SOLID_REACTIVE_NEXUS_math`: a short hue-shifted pulse that follows the
/// hit's row and column.
///
/// Three separate tests force the effect to zero: an un-reached LED (the age
/// term), anything beyond 72 units, and anything more than 8 units off both
/// axes. The hue follows `dy`, so the row lights in a colour that shifts with
/// the distance down the board.
pub fn solid_reactive_nexus_math(hsv: Hsv, dx: i16, dy: i16, dist: u8, tick: u16) -> Hsv {
    let mut effect = tick.wrapping_sub(dist as u16);
    if effect > 255 || dist > 72 || (abs_i16(dx) > 8 && abs_i16(dy) > 8) {
        effect = 255;
    }
    hsv.with_v(qadd8(hsv.v, 255 - effect as u8))
        .with_h(hsv.h.wrapping_add((dy / 4) as u8))
}

/// `SPLASH_math`: a hue-shifting ring expanding away from a hit.
///
/// `tick - dist` wraps in `uint16_t` before the clamp, so a hit that is recent
/// enough (or an LED far enough away) saturates at 255 rather than going
/// negative — which is what makes the wavefront a ring instead of a disc.
pub fn splash_math(hsv: Hsv, _dx: i16, _dy: i16, dist: u8, tick: u16) -> Hsv {
    let effect = (tick.wrapping_sub(dist as u16)).min(255) as u8;
    hsv.with_h(hsv.h.wrapping_add(effect))
        .with_v(qadd8(hsv.v, 255 - effect))
}

/// `SOLID_SPLASH_math`: [`splash_math`] without the hue shift, so the ring is a
/// brightness pulse in the configured colour.
pub fn solid_splash_math(hsv: Hsv, _dx: i16, _dy: i16, dist: u8, tick: u16) -> Hsv {
    let effect = (tick.wrapping_sub(dist as u16)).min(255) as u8;
    hsv.with_v(qadd8(hsv.v, 255 - effect))
}

/// The colour `JELLYBEAN_RAINDROPS` and `PIXEL_RAIN` share: a fully random hue,
/// a saturation in the upper half, and the configured brightness.
///
/// Both C originals spell the same two draws in the same order, so the RNG
/// sequence is observable and this stays one helper rather than two copies.
pub fn random_jellybean_color(config: &RgbConfig, rng: &mut Rng) -> Rgb {
    let hsv = Hsv::new(rng.next8(), rng.next8_min_max(127, 255), config.hsv.v);
    hsv_to_rgb(hsv)
}

/// `JELLYBEAN_RAINDROPS`' colour for one LED.
///
/// The `init` pass runs before the per-frame repaint, so it draws first.
pub fn jellybean_raindrops_set_color(
    index: u8,
    config: &RgbConfig,
    rng: &mut Rng,
    frame: &mut impl LedFrame,
) {
    frame.set_color(index as usize, random_jellybean_color(config, rng));
}

/// `JELLYBEAN_RAINDROPS`: re-colour a random LED on a periodic trigger.
///
/// Differs from [`raindrops`] in two ways: the whole chain is randomised on the
/// first frame, and the repaint picks a new LED outright instead of nudging the
/// previous one's hue. The trigger is `% 5` where `RAINDROPS` uses `% 10`.
///
/// `index` is QMK's `static uint16_t index`, shared with [`raindrops`] because
/// only one of the two can be the selected effect at a time. It is what limits a
/// trigger to one repaint: the previous implementation fired on every batch that
/// happened to land inside the `% 5` window, which re-rolled the LED several
/// times per frame.
#[allow(clippy::manual_is_multiple_of)]
pub fn jellybean_raindrops<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    rng: &mut Rng,
    index: &mut u16,
    frame: &mut F,
) -> bool {
    // The trigger tests `iter == 0`, so it fires once per frame rather than once
    // per batch; the parked index is what makes that once per trigger.
    if params.iter == 0 && scale16by8(ctx.time as u16, qadd8(ctx.config.speed, 16)) % 5 == 0 {
        *index = rng.next8_max(LED_COUNT as u8) as u16;
    }

    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    if params.init {
        for i in led_min..led_max {
            if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
                continue;
            }
            jellybean_raindrops_set_color(i, ctx.config, rng, frame);
        }
    } else if led_min as u16 <= *index && *index < led_max as u16 {
        if has_any_flags(ctx.leds.flags(*index as usize), params.flags) {
            jellybean_raindrops_set_color(*index as u8, ctx.config, rng, frame);
        }
        // Park the index until the next trigger.
        *index = u16::MAX;
    }

    ctx.more_batches(led_max)
}

/// `PIXEL_RAIN`'s cycle length in milliseconds, from the configured speed.
///
/// QMK scales it between 256 ms and 2048 ms: `2048 - scale16by8(1792, speed)`.
/// Faster means a *shorter* wait, which is what the subtraction expresses.
pub fn pixel_rain_interval(speed: u8) -> u32 {
    2048 - scale16by8(1792, speed) as u32
}

/// `PIXEL_RAIN`: randomly blank or re-colour one LED at a time.
///
/// The first draw decides between off and a random colour, so half the cycles
/// darken whatever LED they picked — that is what separates this from
/// `JELLYBEAN_RAINDROPS`, which never turns an LED off.
///
/// The whole repaint is gated on the timer, and the LED is only written by the
/// batch that owns its index. The timer is rearmed and a fresh index drawn on
/// the frame's **last** batch, which is where QMK does it, so the cadence does
/// not depend on which batch the current index happens to fall into.
pub fn pixel_rain<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    rng: &mut Rng,
    index: &mut u8,
    wait_timer: &mut u32,
    frame: &mut F,
) -> bool {
    // `params->iter == 0 && params->init`: a fresh index on the first frame, so
    // the first cycle has a target rather than the zero the field starts at.
    if params.iter == 0 && params.init {
        *index = rng.next8_max(LED_COUNT as u8);
    }

    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let last_batch = !ctx.more_batches(led_max);

    if ctx.time > *wait_timer {
        if led_min as u16 <= *index as u16
            && (*index as u16) < led_max as u16
            && has_any_flags(ctx.leds.flags(*index as usize), params.flags)
        {
            // `(random8() & 2)` is the off/on coin flip; it is drawn either way,
            // so the branch must not skip it.
            let off = (rng.next8() & 2) != 0;
            let rgb = if off {
                Rgb::BLACK
            } else {
                random_jellybean_color(ctx.config, rng)
            };
            frame.set_color(*index as usize, rgb);
        }

        if last_batch {
            // The index and the timer both move on the final LED range, as QMK
            // does, so a cycle advances even when its index carried no LED.
            *index = rng.next8_max(LED_COUNT as u8);
            *wait_timer = ctx.time.wrapping_add(pixel_rain_interval(ctx.config.speed));
        }
    }

    !last_batch
}

/// QMK's `g_rgb_frame_buffer`: one byte of persistent state per matrix cell.
///
/// `TYPING_HEATMAP` keeps a temperature here and `DIGITAL_RAIN` a droplet
/// intensity; both are indexed by the *electrical* matrix position rather than
/// by LED index, which is why the row and column counts travel with the buffer
/// instead of being inferred from the LED chain.
#[derive(Clone, Copy, Debug)]
pub struct MatrixFrame<const ROWS: usize, const COLS: usize> {
    cells: [[u8; COLS]; ROWS],
}

impl<const ROWS: usize, const COLS: usize> MatrixFrame<ROWS, COLS> {
    /// Rows of the electrical matrix, QMK's `MATRIX_ROWS`.
    pub const ROWS: usize = ROWS;
    /// Columns of the electrical matrix, QMK's `MATRIX_COLS`.
    pub const COLS: usize = COLS;

    pub const fn new() -> Self {
        Self {
            cells: [[0; COLS]; ROWS],
        }
    }

    #[inline]
    pub fn get(&self, row: usize, col: usize) -> u8 {
        self.cells[row][col]
    }

    #[inline]
    pub fn set(&mut self, row: usize, col: usize, value: u8) {
        self.cells[row][col] = value;
    }

    /// Add to a cell the way C's `uint8_t +=` does, saturating at 255.
    #[inline]
    pub fn add(&mut self, row: usize, col: usize, amount: u8) {
        self.cells[row][col] = qadd8(self.cells[row][col], amount);
    }

    /// Subtract one from a cell, floored at zero.
    #[inline]
    pub fn decay(&mut self, row: usize, col: usize) {
        let cell = &mut self.cells[row][col];
        *cell = cell.saturating_sub(1);
    }

    pub fn clear(&mut self) {
        self.cells = [[0; COLS]; ROWS];
    }

    /// The raw grid, for copying the state across a shared-state boundary.
    #[inline]
    pub const fn cells(&self) -> &[[u8; COLS]; ROWS] {
        &self.cells
    }

    /// Replace the grid wholesale, the receiving half of [`Self::cells`].
    #[inline]
    pub fn set_cells(&mut self, cells: &[[u8; COLS]; ROWS]) {
        self.cells = *cells;
    }
}

impl<const ROWS: usize, const COLS: usize> Default for MatrixFrame<ROWS, COLS> {
    fn default() -> Self {
        Self::new()
    }
}

/// `RGB_MATRIX_TYPING_HEATMAP_INCREASE_STEP` in QMK's coordinate space.
const HEATMAP_INCREASE_STEP: u8 = 32;
/// `RGB_MATRIX_TYPING_HEATMAP_DECREASE_DELAY_MS`.
pub const HEATMAP_DECREASE_DELAY_MS: u32 = 25;
/// `RGB_MATRIX_TYPING_HEATMAP_SPREAD`, in LED coordinate units (0..=224).
const HEATMAP_SPREAD: u8 = 40;
/// `RGB_MATRIX_TYPING_HEATMAP_AREA_LIMIT`.
const HEATMAP_AREA_LIMIT: u8 = 16;

/// `process_rgb_matrix_typing_heatmap`: heat the hit key and every key whose LED
/// sits within `RGB_MATRIX_TYPING_HEATMAP_SPREAD` of it.
///
/// The spread is measured in LED coordinates rather than in matrix steps, so the
/// heat follows the physical layout: a key diagonally adjacent on a wide board
/// is further away than one directly beside it. The pressed key always heats
/// itself, whatever the layout.
pub fn typing_heatmap_record<const ROWS: usize, const COLS: usize>(
    frame: &mut MatrixFrame<ROWS, COLS>,
    leds: &[LedInfo],
    row: u8,
    col: u8,
) {
    let at = |row: u8, col: u8| -> Option<LedPoint> {
        leds.iter()
            .find(|info| info.matrix_pos == Some((row, col)))
            .map(|info| info.point)
    };
    let Some(origin) = at(row, col) else {
        // A pressed key with no LED under it contributes nothing, as QMK's
        // `matrix_co[row][col] == NO_LED` early return does.
        return;
    };

    // QMK's `LED_DISTANCE`: the same `sqrt16` the distance-based effects use, so
    // the spread is measured in LED coordinate units, not matrix steps.
    let distance = |point: LedPoint| {
        let (dx, dy) = (
            point.x as i32 - origin.x as i32,
            point.y as i32 - origin.y as i32,
        );
        sqrt16((dx * dx + dy * dy) as u16)
    };

    for i_row in 0..ROWS {
        for i_col in 0..COLS {
            let Some(point) = at(i_row as u8, i_col as u8) else {
                continue;
            };
            if i_row as u8 == row && i_col as u8 == col {
                frame.add(i_row, i_col, HEATMAP_INCREASE_STEP);
                continue;
            }
            let distance = distance(point);
            if distance <= HEATMAP_SPREAD {
                frame.add(i_row, i_col, qsub8(HEATMAP_SPREAD, distance).min(HEATMAP_AREA_LIMIT));
            }
        }
    }
}

/// `TYPING_HEATMAP`'s colour for one cell: a hue that walks from green toward
/// blue as the cell heats, and a value scaled from the temperature.
pub fn typing_heatmap_color(val: u8, sat: u8, max_val: u8) -> Rgb {
    let hue = 170u8.wrapping_sub(qsub8(val, 85));
    let value = scale8((qadd8(170, val) - 170) * 3, max_val);
    hsv_to_rgb(Hsv::new(hue, sat, value))
}

/// `TYPING_HEATMAP`: a decaying heat map lit by the keys that were pressed.
///
/// The walk is over *matrix cells*, not LEDs, and a cell maps to its LED through
/// the table's electrical position, so an unpopulated cell simply lights
/// nothing. A cell cools once every [`HEATMAP_DECREASE_DELAY_MS`], not once per
/// frame, which is what makes the fade speed independent of the frame rate.
pub fn typing_heatmap<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame_buf: &mut MatrixFrame<6, 4>,
    timer: &mut DecreaseTimer,
    frame: &mut F,
) -> bool {
    const ROWS: usize = 6;
    const COLS: usize = 4;

    if params.init {
        frame_buf.clear();
        // The keyboard task is the only writer of the heat, and it holds its own
        // copy; clearing only this one would let the next frame's snapshot copy
        // every leftover temperature back in.
        crate::rgb::shared::clear_heat();
        for i in 0..LED_COUNT {
            frame.set_color(i, Rgb::BLACK);
        }
        return false;
    }

    // `heatmap_decrease_timer` marks the last decrease, so the elapsed test is
    // against that rather than against the frame start.
    let due = ctx.time.wrapping_sub(timer.last) >= HEATMAP_DECREASE_DELAY_MS;
    // The frame may run in several batches, so only the first one arms the timer;
    // the rest of the frame reuses the decision it made.
    let decrease = if params.iter == 0 {
        timer.due = due;
        if due {
            timer.last = ctx.time;
        }
        due
    } else {
        timer.due
    };

    let cells = ROWS * COLS;
    let per_batch = ctx.process_limit.max(1);
    let start = per_batch * params.iter as usize;
    let end = (start + per_batch).min(cells);
    if start >= cells {
        return false;
    }

    // QMK batches over LEDs, not cells: an LED outside the current batch is
    // skipped before its cell is rendered, and the cell's heat survives because
    // it was never processed. Walking cells but painting only the LEDs inside
    // the batch keeps both halves of that behaviour.
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);

    for i in start..end {
        let row = i % ROWS;
        let col = i / ROWS;
        let val = frame_buf.get(row, col);

        // One cell can carry more than one LED; QMK lights every one of them.
        let mut leds = [0u8; 8];
        let count = ctx.leds.leds_at(row as u8, col as u8, &mut leds);
        let rgb = typing_heatmap_color(val, ctx.config.hsv.s, ctx.config.hsv.v);
        let mut processed = false;
        for &led in &leds[..count] {
            if (led as usize) < led_min as usize || (led as usize) >= led_max as usize {
                continue;
            }
            if !has_any_flags(ctx.leds.flags(led as usize), params.flags) {
                continue;
            }
            frame.set_color(led as usize, rgb);
            processed = true;
        }

        if processed && decrease {
            frame_buf.decay(row, col);
        }
    }

    end < cells
}

/// `DIGITAL_RAIN`'s colour for one cell: a dim green ramp, saturating to white
/// at the head of a droplet.
///
/// The three levels all derive from the configured brightness, as QMK's do, so
/// the effect follows the value slider and `max_brightness` instead of pinning
/// itself to full scale.
///
/// The divisions are computed in `u32` and narrowed last, because the C carries
/// them in `int` and only truncates on assignment to the `uint8_t` channel.
/// `intensity` is never above `max_intensity` on a real frame — a droplet is
/// spawned at the ceiling and only ever decays from it — so the quotient fits a
/// byte and the narrowing is exact.
pub fn digital_rain_color(intensity: u8, max_intensity: u8) -> Rgb {
    let pure_green_intensity = ((max_intensity as u32) * 3 / 4) as u8;
    let max_brightness_boost = ((max_intensity as u32) * 3 / 4) as u8;

    if intensity > pure_green_intensity {
        let boost = ((max_brightness_boost as u32 * (intensity - pure_green_intensity) as u32)
            / (max_intensity - pure_green_intensity) as u32) as u8;
        Rgb::new(boost, max_intensity, boost)
    } else {
        let green = ((max_intensity as u32 * intensity as u32) / pure_green_intensity as u32) as u8;
        Rgb::new(0, green, 0)
    }
}

/// `DIGITAL_RAIN`: droplets falling down each matrix column.
///
/// The whole grid is walked every frame — unlike the heat map, this effect does
/// not batch — and a column's fall advances once every `DROP_TICKS` frames.
/// `drop` and `decay` are QMK's two `static` counters, carried here so they
/// survive the call.
pub fn digital_rain<const LED_COUNT: usize, F: LedFrame + crate::rgb::runner::FrameAll>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame_buf: &mut MatrixFrame<6, 4>,
    rng: &mut Rng,
    drop: &mut u8,
    decay: &mut u8,
    frame: &mut F,
) -> bool {
    const ROWS: usize = 6;
    const COLS: usize = 4;
    const DROP_TICKS: u8 = 28;
    /// `RGB_DIGITAL_RAIN_DROPS`, the 1-in-N chance of a new droplet per column
    /// per frame. Lower is denser.
    const DROPS: u8 = 24;

    // `max_intensity` is the configured brightness, so a dimmer setting both
    // shortens the trail (`decay_ticks`) and scales every colour.
    let max_intensity = ctx.config.hsv.v;
    if max_intensity == 0 {
        // The division below would be a divide by zero; a black chain is the
        // only thing a zero brightness can render anyway.
        frame.set_all(Rgb::BLACK);
        return false;
    }
    let decay_ticks = 0xff / max_intensity;

    if params.init {
        frame_buf.clear();
        *drop = 0;
        for i in 0..LED_COUNT {
            frame.set_color(i, Rgb::BLACK);
        }
    }

    *decay = decay.wrapping_add(1);

    for col in 0..COLS {
        for row in 0..ROWS {
            // The two branches are QMK's: a spawning column is skipped for
            // decay, everything else that is mid-fade loses one step per
            // `decay_ticks` frames.
            if row == 0 && *drop == 0 && rng.next8_max(DROPS) == 0 {
                // A fresh droplet starts at the top of this column.
                frame_buf.set(row, col, max_intensity);
            } else if *decay == decay_ticks
                && frame_buf.get(row, col) > 0
                && frame_buf.get(row, col) < max_intensity
            {
                frame_buf.decay(row, col);
            }

            let mut leds = [0u8; 8];
            let count = ctx.leds.leds_at(row as u8, col as u8, &mut leds);
            let rgb = digital_rain_color(frame_buf.get(row, col), max_intensity);
            for &led in &leds[..count] {
                if has_any_flags(ctx.leds.flags(led as usize), params.flags) {
                    frame.set_color(led as usize, rgb);
                }
            }
        }
    }

    if *decay == decay_ticks {
        *decay = 0;
    }

    *drop = drop.wrapping_add(1);
    if *drop > DROP_TICKS {
        *drop = 0;
        // Advance every droplet one row downward, bottom row first so a cell is
        // not overwritten before it has been read. The comparison is `>=`
        // because a cell can sit above `max_intensity` if the value slider was
        // lowered while a droplet was falling.
        for row in (1..ROWS).rev() {
            for col in 0..COLS {
                if row == ROWS - 1 && frame_buf.get(row, col) == max_intensity {
                    frame_buf.decay(row, col);
                }
                if frame_buf.get(row - 1, col) >= max_intensity {
                    // The old head keeps a one-step trail rather than vanishing.
                    frame_buf.set(row - 1, col, max_intensity - 1);
                    frame_buf.set(row, col, max_intensity);
                }
            }
        }
    }

    false
}

/// A mirror of QMK's `static bool decrease_heatmap_values`, tracked alongside
/// the timer so the later batches of a frame reuse the first batch's decision.
#[derive(Clone, Copy, Debug, Default)]
pub struct DecreaseTimer {
    /// `heatmap_decrease_timer`: the clock at the last decrease.
    pub last: u32,
    /// Whether the current frame cools the grid.
    pub due: bool,
}

/// The cross-frame state the framebuffer effects keep.
///
/// QMK keeps each piece in a `static` local inside its effect. Bundling them
/// keeps the engine's struct and the dispatch signature from growing a field per
/// effect, and keeps `FractalBits` sized by the same matrix the frame buffer is.
#[derive(Clone, Copy, Debug, Default)]
pub struct FramebufferEffects {
    /// `g_rgb_frame_buffer`: `TYPING_HEATMAP`'s heat.
    pub heat: MatrixFrame<6, 4>,
    /// `DIGITAL_RAIN`'s `static uint8_t drop`.
    pub digital_rain_drop: u8,
    /// `DIGITAL_RAIN`'s `static uint8_t decay`.
    pub digital_rain_decay: u8,
    /// `PIXEL_FRACTAL`'s `static bool led[MATRIX_ROWS][MID_COL]`.
    pub fractal: FractalBits,
    /// `PIXEL_FRACTAL`'s `static uint32_t wait_timer`.
    pub fractal_wait: u32,
    /// `TYPING_HEATMAP`'s decrease timer and its per-frame decision.
    pub heatmap: DecreaseTimer,
}

impl FramebufferEffects {
    pub const fn new() -> Self {
        Self {
            heat: MatrixFrame::new(),
            digital_rain_drop: 0,
            digital_rain_decay: 0,
            fractal: FractalBits::new(),
            fractal_wait: 0,
            heatmap: DecreaseTimer {
                last: 0,
                due: false,
            },
        }
    }
}

/// `PIXEL_FRACTAL`'s state: one bit per row pinning the column it was born in.
///
/// QMK stores `bool led[MATRIX_ROWS][MID_COL]` and shifts each row left by one
/// entry per tick, copying the value out to the mirrored column on the other
/// side. A bit per entry is the same thing without the `bool` array, and the
/// length is derived from the matrix rather than written down twice.
#[derive(Clone, Copy, Debug, Default)]
pub struct FractalBits {
    /// Row `r`'s occupied columns, bit `c` for `MID_COL` entry `c`.
    rows: [u32; 6],
}

impl FractalBits {
    /// `MATRIX_ROWS`, matching the frame buffer's own rows.
    const ROWS: usize = 6;
    /// `MATRIX_COLS`, matching the frame buffer's own columns.
    const COLS: usize = 4;
    /// `MID_COL`, QMK's `(MATRIX_COLS + 1) / 2`.
    const MID_COL: usize = Self::COLS.div_ceil(2);

    pub const fn new() -> Self {
        Self { rows: [0; 6] }
    }

    /// Whether row `row`'s entry `index` is lit, counting from the middle out.
    #[inline]
    fn lit(&self, row: usize, index: usize) -> bool {
        self.rows[row] >> index & 1 != 0
    }

    /// Replace the row's entries with one fresh head, QMK's
    /// `memmove(led[row], led[row] + 1, ...)` plus the new `random8() & 3`.
    fn advance(&mut self, row: usize, head: bool) {
        let mask = (1u32 << Self::MID_COL) - 1;
        self.rows[row] = ((self.rows[row] << 1) | head as u32) & mask;
    }

    fn clear(&mut self) {
        self.rows = [0; 6];
    }

    /// `MATRIX_COLS - 1 - col`, the column mirroring `col` about the centre.
    #[inline]
    const fn mirror(col: usize) -> usize {
        Self::COLS - 1 - col
    }
}

/// `PIXEL_FRACTAL`: a fractal pattern growing outwards from the middle columns.
///
/// The row advances only on the frame's last batch, but *every* batch paints its
/// own LEDs, because a batch is a slice of the chain rather than of the matrix.
/// Advancing on the first batch instead would leave the later batches to be
/// skipped by the timer check, and the LEDs they own would never be written.
pub fn pixel_fractal<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    bits: &mut FractalBits,
    rng: &mut Rng,
    wait_timer: &mut u32,
    frame: &mut F,
) -> bool {
    const ROWS: usize = 6;
    const COLS: usize = 4;
    const MID_COL: usize = FractalBits::MID_COL;

    if params.init {
        bits.clear();
        for i in 0..LED_COUNT {
            frame.set_color(i, Rgb::BLACK);
        }
        return false;
    }

    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    // `rgb_matrix_check_finished_leds`: this call is the frame's last batch, so
    // it is the one allowed to advance the pattern.
    let last_batch = !ctx.more_batches(led_max);

    // The pattern is painted on *every* call, whether or not the timer has fired.
    // QMK's `if (g_rgb_timer > wait_timer)` wraps the paint loop as well, but it
    // also paints every LED in one call; with batching, skipping the paint would
    // leave the LEDs owned by this batch holding whatever the previous frame put
    // there, and nothing would ever come back for them.
    let lit = hsv_to_rgb(ctx.config.hsv);
    for row in 0..ROWS {
        for index in 0..MID_COL {
            let value = if bits.lit(row, index) { lit } else { Rgb::BLACK };
            // QMK writes the left LED first and then the mirror, skipping the
            // mirror when the two name the same LED.
            let left = col_for(index);
            let right = FractalBits::mirror(left);
            paint_cell(ctx, params, frame, led_min, led_max, row, left, value);
            if right != left {
                paint_cell(ctx, params, frame, led_min, led_max, row, right, value);
            }
        }
    }

    if last_batch && ctx.time > *wait_timer {
        for row in 0..ROWS {
            // `(random8() & 3) ? false : true`, so a new head appears one frame
            // in four. The bits are shifted in together with that head, which is
            // what marches the pattern outward from the middle.
            bits.advance(row, rng.next8() & 3 == 0);
        }
        *wait_timer = ctx.time.wrapping_add(pixel_fractal_interval(ctx.config.speed));
    }

    !last_batch
}

/// The matrix column `MID_COL` entry `index` occupies, counting from the middle
/// out towards the left edge.
#[inline]
const fn col_for(index: usize) -> usize {
    FractalBits::MID_COL - 1 - index
}

/// `PIXEL_FRACTAL`'s tick interval in milliseconds, from the configured speed.
pub fn pixel_fractal_interval(speed: u8) -> u32 {
    3000 / scale16by8(qadd8(speed, 16) as u16, 16) as u32
}

/// Light the LED at electrical `(row, col)`, if the board has one there and it
/// falls inside the current batch.
///
/// A board with an LED outside the key matrix (underglow, say) can map it to no
/// cell at all; this is the reverse direction, a cell with no LED under it, and
/// it simply draws nothing.
#[allow(clippy::too_many_arguments)]
fn paint_cell<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
    led_min: u8,
    led_max: u8,
    row: usize,
    col: usize,
    rgb: Rgb,
) {
    let mut leds = [0u8; 8];
    let count = ctx.leds.leds_at(row as u8, col as u8, &mut leds);
    for &led in &leds[..count] {
        if (led as usize) < led_min as usize || (led as usize) >= led_max as usize {
            continue;
        }
        if has_any_flags(ctx.leds.flags(led as usize), params.flags) {
            frame.set_color(led as usize, rgb);
        }
    }
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
            let shift = ((scale as u16 * x as u16) >> 5) as u8;
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
    fn rainbow_pinwheels_folds_the_two_halves_against_each_other() {
        let hsv = Hsv::new(0, 255, 255);
        // Equal distance either side of the centre folds to the same x term.
        let left = rainbow_pinwheels_math(hsv, 100, 0, 0, 32, 112, 32);
        let right = rainbow_pinwheels_math(hsv, 100, 0, 224, 32, 112, 32);
        assert_eq!(left.h, right.h, "the fold mirrors the two halves");

        // A LED on the centre line is unaffected by the x term when sin is 0.
        let centred = rainbow_pinwheels_math(hsv, 0, 0, 112, 32, 112, 32);
        assert_eq!(centred.h, 0);
    }

    #[test]
    fn cycle_out_in_dual_rings_come_from_each_half() {
        let hsv = Hsv::new(0, 255, 255);
        // Folding about centre_x/2 means a LED and its mirror share a ring.
        let a = cycle_out_in_dual_math(hsv, -56, 0, 0, 112);
        let b = cycle_out_in_dual_math(hsv, 56, 0, 0, 112);
        assert_eq!(a.h, b.h, "both halves are folded onto one focus");

        // Moving away from a focus changes the ring.
        let near = cycle_out_in_dual_math(hsv, 0, 0, 0, 112);
        let far = cycle_out_in_dual_math(hsv, 0, 56, 0, 112);
        assert_ne!(near.h, far.h);
    }

    #[test]
    fn rainbow_moving_chevron_bends_away_from_the_centre_line() {
        let hsv = Hsv::new(0, 255, 255);
        // On the centre line the y term vanishes, leaving the sweep term.
        let on_axis = rainbow_moving_chevron_math(hsv, 10, 32, 0, 32);
        assert_eq!(on_axis.h, 10, "x minus time");

        // Off the axis the chevron term is added, symmetrically.
        let above = rainbow_moving_chevron_math(hsv, 10, 0, 0, 32);
        let below = rainbow_moving_chevron_math(hsv, 10, 64, 0, 32);
        assert_eq!(above.h, below.h, "the chevron is symmetric about y");
        assert_ne!(above.h, on_axis.h);
    }

    #[test]
    fn splash_saturates_behind_the_wavefront() {
        // A low starting value, so the brightness term is not already pinned at
        // 255 and the two distances stay distinguishable.
        let hsv = Hsv::new(0, 255, 50);
        // A tick older than the distance means the ring has not arrived yet.
        let behind = splash_math(hsv, 0, 0, 100, 60);
        assert_eq!(behind.h, hsv.h.wrapping_add(255), "clamped to 255");
        assert_eq!(behind.v, qadd8(hsv.v, 0), "nothing added behind the front");

        // Inside the ring the hue shift grows as the distance shrinks, while the
        // brightness does the opposite: `v` gains `255 - effect`, so an LED the
        // wavefront has not reached yet is the brighter one. The tick is kept
        // below 255 so neither distance saturates the shift.
        let near = splash_math(hsv, 0, 0, 10, 150);
        let far = splash_math(hsv, 0, 0, 120, 150);
        assert!(near.h > far.h, "closer to the hit means a larger hue shift");
        assert!(far.v > near.v, "and the un-reached LED is brighter");
    }

    #[test]
    fn solid_splash_keeps_the_hue_and_only_pulses_brightness() {
        let hsv = Hsv::new(77, 255, 200);
        let lit = solid_splash_math(hsv, 0, 0, 10, 200);
        assert_eq!(lit.h, 77, "the configured hue is untouched");
        assert!(lit.v > hsv.v, "the pulse brightens");

        // Far behind the wavefront it adds nothing.
        let dark = solid_splash_math(hsv, 0, 0, 200, 1);
        assert_eq!(dark.v, qadd8(200, 0));
    }

    #[test]
    fn jellybean_colour_keeps_the_brightness_and_varying_hue() {
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(7);

        let first = random_jellybean_color(&config, &mut rng);
        // The value is the configured one, so a full-brightness draw is not black
        // unless the hue/sat pair happens to land there.
        let mut seen_hues = 0;
        let mut last = first;
        for _ in 0..16 {
            let next = random_jellybean_color(&config, &mut rng);
            if next != last {
                seen_hues += 1;
            }
            last = next;
        }
        assert!(seen_hues > 0, "the colour varies between draws");
    }

    #[test]
    fn pixel_rain_interval_shrinks_as_speed_rises() {
        // `2048 - scale16by8(1792, speed)`: the subtracted term grows with speed,
        // so a higher speed is a shorter wait. QMK's own comment calls the range
        // "256–2048 ms"; the truncation in `scale16by8` lands the top at 263.
        assert!(pixel_rain_interval(255) < pixel_rain_interval(0));
        assert_eq!(pixel_rain_interval(0), 2048, "slowest");
        assert_eq!(pixel_rain_interval(255), 263, "fastest");
    }

    #[test]
    fn pixel_rain_repaints_once_the_wait_elapses() {
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(11);
        let leds = Box::leak(Box::new(crate::rgb::led_config::LedConfig::new(
            [crate::rgb::led_config::LedPoint::new(0, 0); 2],
            [0xFF; 2],
            [(0, 0), (0, 1)],
        )));
        let ctx_at = |time| RenderContext {
            leds,
            config: &config,
            center: RenderContext::<2>::DEFAULT_CENTER,
            time,
            process_limit: 2,
        };
        let run = EffectParams { iter: 0, flags: 0xFF, init: false };
        let mut frame = ArrayFrame::<2>::new();
        let mut index = 0u8;

        // Armed at 1000: a tick that has not passed it repaints nothing.
        let mut wait = 1000u32;
        pixel_rain(&ctx_at(999), &run, &mut rng, &mut index, &mut wait, &mut frame);
        assert_eq!(wait, 1000, "still waiting");

        // Crossing it arms the next repaint one interval further out.
        pixel_rain(&ctx_at(1001), &run, &mut rng, &mut index, &mut wait, &mut frame);
        assert_eq!(wait, 1001 + pixel_rain_interval(128), "rearmed after a cycle");
    }

    /// The timer and the index both move on the frame's *last* batch, so the
    /// cadence does not depend on which batch the current index falls into.
    #[test]
    fn pixel_rain_rearms_on_the_last_batch_only() {
        let leds = Box::leak(Box::new(crate::rgb::led_config::LedConfig::new(
            [crate::rgb::led_config::LedPoint::new(0, 0); 6],
            [0xFF; 6],
            [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)],
        )));
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(5);
        let mut index = 0u8;
        let mut wait = 0u32;
        let mut armed_on = Vec::new();

        // Six LEDs at two per batch: three batches, the last one index 2.
        for iter in 0..3u8 {
            let before = wait;
            let ctx = RenderContext {
                leds,
                config: &config,
                center: RenderContext::<6>::DEFAULT_CENTER,
                time: 5000,
                process_limit: 2,
            };
            let params = EffectParams { iter, flags: 0xFF, init: false };
            let mut frame = ArrayFrame::<6>::new();
            pixel_rain(&ctx, &params, &mut rng, &mut index, &mut wait, &mut frame);
            if wait != before {
                armed_on.push(iter);
            }
        }
        assert_eq!(armed_on, vec![2], "only the final batch may rearm the timer");
    }

    /// `JELLYBEAN_RAINDROPS` repaints at most one LED per trigger, and a
    /// multi-batch frame does not turn one trigger into several repaints.
    #[test]
    fn jellybean_raindrops_repaints_once_per_frame() {
        let leds = Box::leak(Box::new(crate::rgb::led_config::LedConfig::new(
            [crate::rgb::led_config::LedPoint::new(0, 0); 6],
            [0xFF; 6],
            [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)],
        )));
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(3);
        let mut index = u16::MAX;
        // `time = 0` puts `scale16by8(0, ..) % 5 == 0`, so the trigger fires.
        let ctx = RenderContext {
            leds,
            config: &config,
            center: RenderContext::<6>::DEFAULT_CENTER,
            time: 0,
            process_limit: 2,
        };

        let mut painted = 0;
        for iter in 0..3u8 {
            let params = EffectParams { iter, flags: 0xFF, init: false };
            let mut frame = ArrayFrame::<6>::new();
            jellybean_raindrops(&ctx, &params, &mut rng, &mut index, &mut frame);
            painted += frame.pixels.iter().filter(|p| **p != Rgb::BLACK).count();
        }
        assert_eq!(painted, 1, "one trigger repaints one LED, not one per batch");
        assert_eq!(index, u16::MAX, "the index is parked between triggers");
    }

    #[test]
    fn solid_reactive_wide_fades_with_five_times_the_distance() {
        let hsv = Hsv::new(0, 255, 50);
        // The same age, further away, is dimmer.
        let near = solid_reactive_wide_math(hsv, 0, 0, 0, 0);
        let far = solid_reactive_wide_math(hsv, 0, 0, 20, 0);
        assert!(near.v > far.v, "distance multiplies the age by five");
        // Beyond 51 units the effect is pinned at its floor.
        assert_eq!(solid_reactive_wide_math(hsv, 0, 0, 255, 255).v, qadd8(hsv.v, 0));
    }

    #[test]
    fn solid_reactive_cross_takes_the_smaller_spread() {
        let hsv = Hsv::new(0, 255, 50);
        // `effect += min(spread_x, spread_y)`, so an LED on either axis (one
        // spread is zero) keeps the pulse no matter how far along that axis it
        // sits. That is what extends the light into a cross.
        let far_along_row = solid_reactive_cross_math(hsv, 100, 0, 0, 0);
        let far_along_col = solid_reactive_cross_math(hsv, 0, 100, 0, 0);
        assert_eq!(far_along_row.v, 255, "the whole row stays lit");
        assert_eq!(far_along_col.v, 255, "the whole column stays lit");

        // Off both axes the larger of the two spreads survives, so the light
        // falls off as the LED leaves the cross.
        let near = solid_reactive_cross_math(hsv, 4, 4, 0, 0);
        let far = solid_reactive_cross_math(hsv, 16, 16, 0, 0);
        assert!(near.v > far.v, "leaving the cross dims the pulse");
    }

    #[test]
    fn solid_reactive_nexus_drops_off_the_cross_and_shifts_hue_by_row() {
        let hsv = Hsv::new(0, 255, 50);
        // Within 8 units of both axes the pulse is lit.
        let lit = solid_reactive_nexus_math(hsv, 2, 2, 0, 100);
        assert!(lit.v > hsv.v, "close to the hit it brightens");
        assert_eq!(lit.h, hsv.h, "a small dy rounds to no hue shift");

        // More than 8 units off both axes is forced to the floor.
        let off = solid_reactive_nexus_math(hsv, 40, 40, 0, 100);
        assert_eq!(off.v, qadd8(hsv.v, 0), "off the cross it adds nothing");

        // Beyond 72 units of distance, likewise.
        let far = solid_reactive_nexus_math(hsv, 0, 0, 73, 100);
        assert_eq!(far.v, qadd8(hsv.v, 0));

        // The hue follows dy / 4.
        let below = solid_reactive_nexus_math(hsv, 0, 40, 0, 100);
        assert_eq!(below.h, hsv.h.wrapping_add(10));
    }

    /// Selecting the heat map clears whatever heat had built up, so the effect
    /// always starts cold. `params.init` is set on an effect change, which is
    /// the engine's signal that this is the first frame of the effect.
    #[test]
    fn typing_heatmap_starts_cold_on_selection() {
        use crate::rgb::led_config::{LedInfo, LedPoint};

        let leds = Box::leak(Box::new(LedConfig::from_matrix(
            [LedPoint::new(0, 0); 2],
            [(0, 0), (1, 1)],
        )));
        // The heat recorder reads the flattened table the shared state keeps.
        let table: Vec<LedInfo> = (0..2).map(|i| leds.info(i)).collect();

        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let ctx = RenderContext {
            leds,
            config: &config,
            center: RenderContext::<2>::DEFAULT_CENTER,
            time: 0,
            process_limit: 2,
        };

        // Presses before the effect is selected leave heat behind.
        let mut frame_buf = MatrixFrame::<6, 4>::new();
        typing_heatmap_record(&mut frame_buf, &table, 0, 0);
        assert!(frame_buf.get(0, 0) > 0, "the press heated the cell");

        // The first frame of the effect clears it.
        let params = EffectParams { iter: 0, flags: 0xFF, init: true };
        let mut timer = DecreaseTimer::default();
        let mut frame = ArrayFrame::<2>::new();
        typing_heatmap(&ctx, &params, &mut frame_buf, &mut timer, &mut frame);

        for row in 0..6 {
            for col in 0..4 {
                assert_eq!(frame_buf.get(row, col), 0, "cell ({row},{col}) was not cooled");
            }
        }
    }

    /// `DIGITAL_RAIN` must eventually light every column, not just one.
    ///
    /// The board's real 19 matrix positions are used, because the effect walks
    /// the *electrical* grid and finds each cell's LED through `matrix_pos`; a
    /// wrong table would show up here as columns that never light.
    #[test]
    fn digital_rain_lights_every_column() {
        use crate::rgb::led_config::{LedConfig, LedPoint};
        use crate::rgb::runner::ArrayFrame;

        // The 19 electrical positions of the real numpad.
        const POS: [(u8, u8); 19] = [
            (0, 0),
            (0, 1),
            (1, 0),
            (1, 1),
            (1, 2),
            (1, 3),
            (2, 0),
            (2, 1),
            (2, 2),
            (3, 0),
            (3, 1),
            (3, 2),
            (3, 3),
            (4, 0),
            (4, 1),
            (4, 2),
            (5, 0),
            (5, 2),
            (5, 3),
        ];
        const N: usize = POS.len();

        let mut points = [LedPoint::new(0, 0); N];
        let mut matrix = [(0u8, 0u8); N];
        for (i, &(row, col)) in POS.iter().enumerate() {
            points[i] = LedPoint::new(col * 44, row * 44);
            matrix[i] = (row, col);
        }
        let leds = Box::leak(Box::new(LedConfig::from_matrix(points, matrix)));

        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(2024);
        let mut frame_buf = MatrixFrame::<6, 4>::new();
        let mut drop = 0u8;
        let mut decay = 0u8;
        let mut lit_cols = [false; 4];

        for frame in 0..4000u32 {
            let ctx = RenderContext {
                leds,
                config: &config,
                center: RenderContext::<N>::DEFAULT_CENTER,
                time: frame,
                process_limit: 4,
            };
            let params = EffectParams {
                iter: 0,
                flags: 0xFF,
                init: frame == 0,
            };
            let mut frame_buf_out = ArrayFrame::<N>::new();
            digital_rain(
                &ctx,
                &params,
                &mut frame_buf,
                &mut rng,
                &mut drop,
                &mut decay,
                &mut frame_buf_out,
            );

            for (i, &(_, col)) in POS.iter().enumerate() {
                let p = frame_buf_out.pixels[i];
                if p != Rgb::BLACK {
                    lit_cols[col as usize] = true;
                }
            }
        }

        assert!(
            lit_cols.iter().all(|&lit| lit),
            "every column should light at some point, got {lit_cols:?}\n\
             final frame_buf by column: {:?}",
            (0..4)
                .map(|c| (0..6).map(|r| frame_buf.get(r, c)).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }

    /// The two beacons are distinct effects, not aliases: `DUAL_BEACON` omits the
    /// `* 2` on the position terms.
    ///
    /// Small offsets keep the arithmetic checkable by hand: with `dy = 8` and
    /// `dx = 8`, and the runner's pair `(cos, sin) = (127, 0)`,
    /// `RAINBOW_BEACON` gives `((8*2*127) + (8*2*0)) / 128 == 15` while
    /// `DUAL_BEACON` gives `((8*127) + (8*0)) / 128 == 7`.
    #[test]
    fn dual_beacon_is_twice_as_coarse_as_rainbow_beacon() {
        let hsv = Hsv::new(0, 255, 255);
        let (cos_arg, sin_arg, x, y, center) = (127i8, 0i8, 120u8, 40u8, 32u8);
        let at = (x, y);

        let dual = dual_beacon_math(hsv, cos_arg, sin_arg, at.0, at.1, 112, center);
        let rainbow = rainbow_beacon_math(hsv, cos_arg, sin_arg, at.0, at.1, 112, center);
        assert_ne!(dual.h, rainbow.h, "the two beacons must not be the same effect");
        // `dy = 8`, `dx = 8`; the sine is zero, so only the `dy` term survives.
        assert_eq!(dual.h, ((8 * 127) / 128) as u8);
        assert_eq!(rainbow.h, ((8 * 2 * 127) / 128) as u8);
        // A centred LED has no offset in either.
        assert_eq!(dual_beacon_math(hsv, cos_arg, sin_arg, 112, center, 112, center).h, 0);
    }

    /// `time = 0` is the case that exposes a swapped `sin`/`cos`: the sine term
    /// vanishes, so an effect that multiplies the x term by the wrong value
    /// collapses onto the y axis instead of sweeping.
    #[test]
    fn sin_cos_effects_vanish_along_the_axis_the_sine_does_not_reach() {
        let hsv = Hsv::new(0, 255, 255);
        // `sin8(0) - 128 == 0`, `cos8(0) - 128 == 127`.
        let (cos_arg, sin_arg) = (127i8, 0i8);

        // On the centre row, `dy == 0`, so only the sine term can move the hue.
        // With the sine at zero the whole row is unshifted.
        assert_eq!(rainbow_beacon_math(hsv, cos_arg, sin_arg, 224, 32, 112, 32).h, 0);
        // Moving along y brings the cosine term in.
        assert_ne!(rainbow_beacon_math(hsv, cos_arg, sin_arg, 112, 64, 112, 32).h, 0);
        // A non-zero sine moves the centre row instead.
        assert_ne!(rainbow_beacon_math(hsv, 0, 100, 224, 32, 112, 32).h, 0);
    }

    /// Parity check against the QMK formula, evaluated by hand.
    ///
    /// `RAINBOW_BEACON_math` is `(dy*2*cos + dx*2*sin) / 128`. The runner's first
    /// argument is the cosine, so these are the values QMK produces at
    /// `time = 32`, where `cos8(32) - 128 == 0` and `sin8(32) - 128 == 127`:
    /// a LED at `(224, 32)` with centre `(112, 32)` has `dy == 0`, so the result
    /// is `(112 * 2 * 127) / 128 == 222`.
    #[test]
    fn rainbow_beacon_matches_the_qmk_formula_at_a_known_phase() {
        let hsv = Hsv::new(0, 255, 255);
        // The runner supplies `(cos_value, sin_value)`, i.e. `(0, 127)` at t=32.
        let beacon = rainbow_beacon_math(hsv, 0, 127, 224, 32, 112, 32);
        assert_eq!(beacon.h, ((112 * 2 * 127) / 128) as u8, "dy == 0 leaves the sine term");

        // With `cos == 127` and `sin == 0` the axes trade over: now only the
        // cosine term lives, and it acts on dy.
        let on_column = rainbow_beacon_math(hsv, 127, 0, 112, 64, 112, 32);
        assert_eq!(on_column.h, (((64 - 32) * 2 * 127) / 128) as u8);
        assert_eq!(rainbow_beacon_math(hsv, 127, 0, 224, 32, 112, 32).h, 0, "dx does not move it");

        // `DIAL_BEACON` drops the `* 2`, so it is exactly half at this phase.
        assert_eq!(dual_beacon_math(hsv, 0, 127, 224, 32, 112, 32).h, ((112 * 127) / 128) as u8);
    }

    /// Every LED the key matrix can address must be reachable by the effect.
    ///
    /// These are the real 19 positions, and `col_for` / `mirror` between them
    /// must cover all four columns of a row that has one.
    #[test]
    fn fractal_columns_cover_the_whole_row() {
        let covered: Vec<usize> = (0..FractalBits::MID_COL)
            .flat_map(|index| {
                let left = col_for(index);
                let right = FractalBits::mirror(left);
                if right == left { vec![left] } else { vec![left, right] }
            })
            .collect();
        let unique: std::collections::BTreeSet<usize> = covered.iter().copied().collect();
        assert_eq!(
            unique,
            (0..FractalBits::COLS).collect(),
            "columns reachable by index 0..MID_COL, got {covered:?}"
        );
    }

    /// `PIXEL_FRACTAL` must write *every* LED over a full wait period, not just
    /// the ones in the frame's first batch.
    ///
    /// The row only advances on the last batch, but each batch paints its own
    /// LEDs; getting that backwards leaves everything past the first batch stale,
    /// which is what a "only four LEDs move" report looks like on hardware.
    #[test]
    fn pixel_fractal_paints_every_led_across_a_frames_batches() {
        use crate::rgb::led_config::{LedConfig, LedPoint};
        use std::collections::BTreeSet;

        // The real 19-position numpad, so the batch slicing lands on the LED
        // indices a board would have.
        const POS: [(u8, u8); 19] = [
            (0, 0),
            (0, 1),
            (1, 0),
            (1, 1),
            (1, 2),
            (1, 3),
            (2, 0),
            (2, 1),
            (2, 2),
            (3, 0),
            (3, 1),
            (3, 2),
            (3, 3),
            (4, 0),
            (4, 1),
            (4, 2),
            (5, 0),
            (5, 2),
            (5, 3),
        ];
        const N: usize = POS.len();

        let mut points = [LedPoint::new(0, 0); N];
        let mut matrix = [(0u8, 0u8); N];
        for (i, &(row, col)) in POS.iter().enumerate() {
            points[i] = LedPoint::new(col * 44, row * 44);
            matrix[i] = (row, col);
        }
        let leds = Box::leak(Box::new(LedConfig::from_matrix(points, matrix)));

        /// Records which LED indices an effect actually wrote.
        #[derive(Default)]
        struct Tracker {
            written: Vec<usize>,
        }

        impl LedFrame for Tracker {
            fn set_color(&mut self, index: usize, _rgb: Rgb) {
                self.written.push(index);
            }
        }

        // 19 LEDs at QMK's default process limit of 4: five batches per frame.
        // `init` is per *frame*, not per batch, so it is consumed once below and
        // the batches that follow it are the ones this test is about.
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(99);
        let mut bits = FractalBits::new();
        let mut wait = 0u32;
        let mut painted: BTreeSet<usize> = BTreeSet::new();

        let ctx = RenderContext {
            leds,
            config: &config,
            center: RenderContext::<N>::DEFAULT_CENTER,
            time: 1000,
            process_limit: 4,
        };

        // The init pass clears the chain, reports completion and leaves the timer
        // alone; it owns no batches.
        let mut frame = Tracker::default();
        assert!(
            !pixel_fractal(
                &ctx,
                &EffectParams {
                    iter: 0,
                    flags: 0xFF,
                    init: true
                },
                &mut bits,
                &mut rng,
                &mut wait,
                &mut frame
            ),
            "the init pass is a single call"
        );
        assert_eq!(wait, 0, "the timer is untouched while initialising");

        // One frame, walked batch by batch exactly as the engine walks it.
        let mut iter = 0u8;
        let mut batches = 0;
        loop {
            let params = EffectParams {
                iter,
                flags: 0xFF,
                init: false,
            };
            let mut frame = Tracker::default();
            let more = pixel_fractal(&ctx, &params, &mut bits, &mut rng, &mut wait, &mut frame);
            painted.extend(frame.written);

            batches += 1;
            if !more {
                break;
            }
            iter += 1;
            assert!(iter < 16, "batching must terminate");
        }

        assert!(batches > 1, "19 LEDs at 4 per batch is a multi-batch frame, got {batches}");
        let missing: Vec<usize> = (0..N).filter(|i| !painted.contains(i)).collect();
        assert!(
            missing.is_empty(),
            "every LED must be written in some batch; missing {missing:?} at positions {:?}",
            missing.iter().map(|&i| POS[i]).collect::<Vec<_>>()
        );
    }

    /// The pattern marches one entry per *frame*, not one per batch.
    ///
    /// A row's head is only re-rolled when the frame's last batch renders, so a
    /// five-batch frame advances the pattern once.
    #[test]
    fn pixel_fractal_advances_once_per_frame_and_only_on_the_last_batch() {
        use crate::rgb::led_config::{LedConfig, LedPoint};
        use crate::rgb::runner::ArrayFrame;

        let leds = LedConfig::from_matrix(
            [
                LedPoint::new(0, 0),
                LedPoint::new(44, 0),
                LedPoint::new(88, 0),
                LedPoint::new(132, 0),
                LedPoint::new(176, 0),
                LedPoint::new(220, 0),
            ],
            [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)],
        );
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut rng = Rng::with_seed(7);
        let mut bits = FractalBits::new();
        let mut wait = 0u32;
        let mut frames_advanced = 0;

        for frame in 0..8u32 {
            let ctx = RenderContext {
                leds: &leds,
                config: &config,
                center: RenderContext::<6>::DEFAULT_CENTER,
                time: 1000 + frame * 10_000,
                process_limit: 2,
            };
            // Snapshot the wait timer: only the frame's last batch may move it.
            let before = wait;
            let mut armed_on: Option<u8> = None;
            let mut iter = 0u8;
            let mut batches = 0;
            loop {
                let params = EffectParams {
                    iter,
                    flags: 0xFF,
                    init: false,
                };
                let mut frame = ArrayFrame::<6>::new();
                let more = pixel_fractal(&ctx, &params, &mut bits, &mut rng, &mut wait, &mut frame);
                if wait != before {
                    assert!(armed_on.is_none(), "the timer was re-armed twice in one frame");
                    armed_on = Some(iter);
                }
                batches += 1;
                if !more {
                    break;
                }
                iter += 1;
            }

            if let Some(iter) = armed_on {
                frames_advanced += 1;
                // Six LEDs at two per batch: the last batch is index 2. Arming on
                // any earlier batch would leave the later LEDs unpainted.
                assert_eq!(iter, batches - 1, "the timer moved before the last batch");
            }
        }

        assert_eq!(frames_advanced, 8, "each frame of the walk should advance the pattern");
    }

    /// `DIGITAL_RAIN`'s scale follows the configured brightness: a lower value
    /// dims the whole effect rather than only the global scale.
    #[test]
    fn digital_rain_colours_follow_the_configured_brightness() {
        // QMK: max_intensity = v, pure_green = boost = 3v/4, so a head at the
        // maximum intensity is `(3v/4, v, 3v/4)` — `3 * 255 >> 2 == 191`.
        let bright = digital_rain_color(255, 255);
        assert_eq!(bright, Rgb::new(191, 255, 191));

        let dim = digital_rain_color(100, 100);
        assert_eq!(dim, Rgb::new(75, 100, 75), "the whole head follows the ceiling down");

        // The green ramp below the threshold is `v * intensity / (3v/4)`, which
        // cancels to `4 * intensity / 3`: the same at either ceiling, because the
        // pure-green reference scales with the brightness too.
        assert_eq!(digital_rain_color(51, 255).g, 68);
        assert_eq!(digital_rain_color(51, 100).g, 68);

        // At the threshold the boost is zero, so the head is pure green.
        assert_eq!(digital_rain_color(191, 255).r, 0);
        assert_eq!(digital_rain_color(75, 100).r, 0);
    }

    /// The heat spread is measured in LED coordinates, not matrix steps: a key
    /// further than `RGB_MATRIX_TYPING_HEATMAP_SPREAD` away is left cold, and one
    /// inside it is warmed by `spread - distance`.
    #[test]
    fn typing_heatmap_spreads_by_distance_not_by_matrix_adjacency() {
        use crate::rgb::led_config::{LedConfig, LedInfo, LedPoint};

        // Three keys in one row, 40 units apart. The spread is 40, so the near
        // neighbour (40 away) is at the limit and the far one (80 away) is out.
        let leds = LedConfig::from_matrix(
            [
                LedPoint::new(0, 0),
                LedPoint::new(40, 0),
                LedPoint::new(80, 0),
            ],
            [(0, 0), (0, 1), (0, 2)],
        );
        let table: Vec<LedInfo> = (0..3).map(|i| leds.info(i)).collect();

        let mut frame = MatrixFrame::<6, 4>::new();
        typing_heatmap_record(&mut frame, &table, 0, 0);

        assert_eq!(frame.get(0, 0), 32, "the pressed key takes the full step");
        assert_eq!(frame.get(0, 1), 0, "a neighbour exactly at the spread adds nothing");
        assert_eq!(frame.get(0, 2), 0, "a key beyond the spread is never warmed");

        // A closer neighbour is warmed by `spread - distance`, capped at 16.
        let closer = LedConfig::from_matrix(
            [LedPoint::new(0, 0), LedPoint::new(10, 0)],
            [(0, 0), (0, 1)],
        );
        let closer_table: Vec<LedInfo> = (0..2).map(|i| closer.info(i)).collect();
        let mut frame = MatrixFrame::<6, 4>::new();
        typing_heatmap_record(&mut frame, &closer_table, 0, 0);
        assert_eq!(frame.get(0, 1), 16, "qsub8(40, 10) caps at the area limit");

        // A key with no LED under it records nothing at all.
        let mut frame = MatrixFrame::<6, 4>::new();
        typing_heatmap_record(&mut frame, &table, 5, 3);
        assert_eq!(frame.get(5, 3), 0);
    }

    /// A cell cools once per `HEATMAP_DECREASE_DELAY_MS`, not once per call.
    #[test]
    fn typing_heatmap_cools_on_the_decrease_delay_not_every_frame() {
        use crate::rgb::led_config::{LedConfig, LedPoint};

        let leds = LedConfig::from_matrix([LedPoint::new(0, 0)], [(0, 0)]);
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 255);
        let mut frame_buf = MatrixFrame::<6, 4>::new();
        frame_buf.set(0, 0, 100);
        let mut timer = DecreaseTimer::default();
        let params = EffectParams { iter: 0, flags: 0xFF, init: false };
        let mut frame = ArrayFrame::<1>::new();

        let ctx_at = |time| RenderContext {
            leds: &leds,
            config: &config,
            center: RenderContext::<1>::DEFAULT_CENTER,
            time,
            process_limit: 1,
        };

        // Same millisecond: the delay has not elapsed, so the cell holds.
        typing_heatmap(&ctx_at(0), &params, &mut frame_buf, &mut timer, &mut frame);
        assert_eq!(frame_buf.get(0, 0), 100, "no cooling inside the delay");

        // The delay after the last decrease: the cell cools by one.
        typing_heatmap(&ctx_at(HEATMAP_DECREASE_DELAY_MS), &params, &mut frame_buf, &mut timer, &mut frame);
        assert_eq!(frame_buf.get(0, 0), 99, "cooled once the delay elapsed");
        assert_eq!(timer.last, HEATMAP_DECREASE_DELAY_MS, "the timer restarts on a decrease");

        // A frame inside the new delay leaves it alone.
        typing_heatmap(
            &ctx_at(HEATMAP_DECREASE_DELAY_MS + 1),
            &params,
            &mut frame_buf,
            &mut timer,
            &mut frame,
        );
        assert_eq!(frame_buf.get(0, 0), 99, "not cooled again immediately");
    }

    /// A zero brightness must not divide by zero, and must render dark.
    #[test]
    fn digital_rain_with_zero_brightness_is_black_and_does_not_panic() {
        use crate::rgb::led_config::{LedConfig, LedPoint};
        use crate::rgb::runner::ArrayFrame;

        let leds = LedConfig::from_matrix([LedPoint::new(0, 0)], [(0, 0)]);
        let config = RgbConfig::with_defaults(true, 100, 200, 128, 0xFF, 0);
        let ctx = RenderContext {
            leds: &leds,
            config: &config,
            center: RenderContext::<1>::DEFAULT_CENTER,
            time: 100,
            process_limit: 1,
        };
        let mut rng = Rng::with_seed(1);
        let mut frame_buf = MatrixFrame::<6, 4>::new();
        let mut drop = 0u8;
        let mut decay = 0u8;
        let mut frame = ArrayFrame::<1>::new();
        frame.fill(Rgb::WHITE);

        let params = EffectParams { iter: 0, flags: 0xFF, init: false };
        digital_rain(&ctx, &params, &mut frame_buf, &mut rng, &mut drop, &mut decay, &mut frame);
        assert_eq!(frame.pixels[0], Rgb::BLACK);
    }

    /// A `FrameAll` frame, so `digital_rain`'s zero-brightness path compiles.
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
