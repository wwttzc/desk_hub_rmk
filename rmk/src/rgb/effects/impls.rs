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

/// `RAINBOW_PINWHEELS_math`: two hue gradients spinning opposite halves.
///
/// The x term folds about the centre (`56 - abs8(dx)`), which is what makes the
/// two halves counter-rotate; `56` is 224/4, hard-coded in QMK.
pub fn rainbow_pinwheels_math(
    hsv: Hsv,
    cos: i8,
    sin: i8,
    x: u8,
    y: u8,
    center_x: u8,
    center_y: u8,
) -> Hsv {
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
#[allow(clippy::manual_is_multiple_of)]
pub fn jellybean_raindrops<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    rng: &mut Rng,
    frame: &mut F,
) -> bool {
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);

    if params.init {
        for i in led_min..led_max {
            if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
                continue;
            }
            jellybean_raindrops_set_color(i, ctx.config, rng, frame);
        }
    } else if scale16by8(ctx.time as u16, qadd8(ctx.config.speed, 16)) % 5 == 0 {
        let index = rng.next8_max(LED_COUNT as u8);
        // The index draw happens before the colour draw, so the flag test has to
        // come after it to keep the two effects' RNG sequences comparable.
        if has_any_flags(ctx.leds.flags(index as usize), params.flags) {
            jellybean_raindrops_set_color(index, ctx.config, rng, frame);
        }
    }

    ctx.more_batches(led_max)
}

/// `PIXEL_RAIN`'s repaint interval in milliseconds, from the configured speed.
///
/// `qadd8(speed, 16)` is at least 16, so the divisor can never be zero.
pub fn pixel_rain_interval(speed: u8) -> u32 {
    500 / scale16by8(qadd8(speed, 16) as u16, 16) as u32
}

/// `PIXEL_RAIN`: randomly blank or re-colour one LED at a time.
///
/// The first draw decides between off and a random colour, so half the frames
/// darken whatever LED they picked — that is what separates this from
/// `JELLYBEAN_RAINDROPS`, which never turns an LED off.
///
/// The effect paints at most one LED and finishes the frame either way, so it
/// never asks for another batch: the batching loop is for effects that walk the
/// chain, and this one is driven by its own timer instead.
pub fn pixel_rain<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    rng: &mut Rng,
    wait_timer: &mut u32,
    frame: &mut F,
) -> bool {
    // QMK leaves a freshly selected effect painting onto the previous frame
    // unless the effect clears it; this one has no `init` branch of its own, so
    // seed the chain once rather than inheriting whatever was lit before.
    if params.init {
        let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
        for i in led_min..led_max {
            if !has_any_flags(ctx.leds.flags(i as usize), params.flags) {
                continue;
            }
            frame.set_color(i as usize, random_jellybean_color(ctx.config, rng));
        }
        return (led_max as usize) < LED_COUNT;
    }

    if ctx.time > *wait_timer {
        let index = rng.next8_max(LED_COUNT as u8);
        if has_any_flags(ctx.leds.flags(index as usize), params.flags) {
            // `(random8() & 2)` is the off/on coin flip; it is drawn either way,
            // so the branch must not skip it.
            let off = (rng.next8() & 2) != 0;
            let rgb = if off {
                Rgb::BLACK
            } else {
                random_jellybean_color(ctx.config, rng)
            };
            frame.set_color(index as usize, rgb);
            *wait_timer = ctx.time.wrapping_add(pixel_rain_interval(ctx.config.speed));
        }
    }

    false
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
}

impl<const ROWS: usize, const COLS: usize> Default for MatrixFrame<ROWS, COLS> {
    fn default() -> Self {
        Self::new()
    }
}

/// `process_rgb_matrix_typing_heatmap`: heat the hit cell and its neighbours.
///
/// The C warms the centre by 32, the four orthogonal neighbours by 16 and the
/// four diagonals by 13. Adding 1 to the coordinates makes the edges wrap to 0
/// and fail the bounds test rather than underflowing — the `wrapping_add` here
/// is what preserves that.
pub fn typing_heatmap_record<const ROWS: usize, const COLS: usize>(
    frame: &mut MatrixFrame<ROWS, COLS>,
    row: u8,
    col: u8,
) {
    let (row, col) = (row as usize, col as usize);
    let (m_row, m_col) = (row.wrapping_sub(1), col.wrapping_sub(1));
    let (p_row, p_col) = (row + 1, col + 1);
    let has_prev_col = m_col < col;
    let has_next_col = p_col < COLS;
    let has_prev_row = m_row < row;
    let has_next_row = p_row < ROWS;

    if has_prev_col {
        frame.add(row, m_col, 16);
    }
    frame.add(row, col, 32);
    if has_next_col {
        frame.add(row, p_col, 16);
    }

    if has_next_row {
        if has_prev_col {
            frame.add(p_row, m_col, 13);
        }
        frame.add(p_row, col, 16);
        if has_next_col {
            frame.add(p_row, p_col, 13);
        }
    }

    if has_prev_row {
        if has_prev_col {
            frame.add(m_row, m_col, 13);
        }
        frame.add(m_row, col, 16);
        if has_next_col {
            frame.add(m_row, p_col, 13);
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
/// nothing. Every visited cell cools by one per frame.
pub fn typing_heatmap<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame_buf: &mut MatrixFrame<6, 4>,
    frame: &mut F,
) -> bool {
    const ROWS: usize = 6;
    const COLS: usize = 4;

    if params.init {
        frame_buf.clear();
        for i in 0..LED_COUNT {
            frame.set_color(i, Rgb::BLACK);
        }
        return false;
    }

    let cells = ROWS * COLS;
    let per_batch = ctx.process_limit.max(1);
    let start = per_batch * params.iter as usize;
    let end = (start + per_batch).min(cells);
    if start >= cells {
        return false;
    }

    for i in start..end {
        let row = i % ROWS;
        let col = i / ROWS;
        let val = frame_buf.get(row, col);

        // One cell can carry more than one LED; QMK lights every one of them.
        let mut leds = [0u8; 8];
        let count = ctx.leds.leds_at(row as u8, col as u8, &mut leds);
        let rgb = typing_heatmap_color(val, ctx.config.hsv.s, ctx.config.hsv.v);
        for &led in &leds[..count] {
            if has_any_flags(ctx.leds.flags(led as usize), params.flags) {
                frame.set_color(led as usize, rgb);
            }
        }

        frame_buf.decay(row, col);
    }

    end < cells
}

/// `DIGITAL_RAIN`'s colour for one cell: a dim green ramp, saturating to white
/// at the head of a droplet.
pub fn digital_rain_color(intensity: u8) -> Rgb {
    const PURE_GREEN_INTENSITY: u8 = 0xd0;
    const MAX_BRIGHTNESS_BOOST: u8 = 0xc0;
    const MAX_INTENSITY: u8 = 0xff;

    if intensity > PURE_GREEN_INTENSITY {
        let boost = (MAX_BRIGHTNESS_BOOST as u16 * (intensity - PURE_GREEN_INTENSITY) as u16
            / (MAX_INTENSITY - PURE_GREEN_INTENSITY) as u16) as u8;
        Rgb::new(boost, MAX_INTENSITY, boost)
    } else {
        let green = (MAX_INTENSITY as u16 * intensity as u16 / PURE_GREEN_INTENSITY as u16) as u8;
        Rgb::new(0, green, 0)
    }
}

/// `DIGITAL_RAIN`: droplets falling down each matrix column.
///
/// The whole grid is walked every frame — unlike the heat map, this effect does
/// not batch — and a column's fall advances once every `DROP_TICKS` frames.
/// `drop` is QMK's `static uint8_t drop`, carried here so it survives the call.
pub fn digital_rain<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame_buf: &mut MatrixFrame<6, 4>,
    rng: &mut Rng,
    drop: &mut u8,
    frame: &mut F,
) -> bool {
    const ROWS: usize = 6;
    const COLS: usize = 4;
    const DROP_TICKS: u8 = 28;
    const MAX_INTENSITY: u8 = 0xff;
    /// `RGB_DIGITAL_RAIN_DROPS`, the 1-in-N chance of a new droplet per column
    /// per frame. Lower is denser.
    const DROPS: u16 = 24;

    if params.init {
        frame_buf.clear();
        *drop = 0;
        for i in 0..LED_COUNT {
            frame.set_color(i, Rgb::BLACK);
        }
        return false;
    }

    // Diagnostic: count where droplets are actually spawned.
    static SPAWNED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    for col in 0..COLS {
        for row in 0..ROWS {
            let cell = frame_buf.get(row, col);
            if row == 0 && *drop == 0 && rng.next8_max(DROPS as u8) == 0 {
                // A fresh droplet starts at the top of this column.
                frame_buf.set(row, col, MAX_INTENSITY);
            } else if cell > 0 && cell < MAX_INTENSITY {
                frame_buf.decay(row, col);
            }

            let mut leds = [0u8; 8];
            let count = ctx.leds.leds_at(row as u8, col as u8, &mut leds);
            if count > 0 {
                // QMK lights only `led[0]` here; multiple LEDs on one cell are
                // still a TODO upstream.
                let led = leds[0];
                if has_any_flags(ctx.leds.flags(led as usize), params.flags) {
                    frame.set_color(led as usize, digital_rain_color(frame_buf.get(row, col)));
                }
            }
        }
    }

    *drop += 1;
    if *drop > DROP_TICKS {
        *drop = 0;
        // Advance every droplet one row downward, bottom row first so a cell is
        // not overwritten before it has been read.
        for row in (1..ROWS).rev() {
            for col in 0..COLS {
                if row == ROWS - 1 && frame_buf.get(row, col) == MAX_INTENSITY {
                    frame_buf.decay(row, col);
                }
                if frame_buf.get(row - 1, col) == MAX_INTENSITY {
                    frame_buf.decay(row - 1, col);
                    frame_buf.set(row, col, MAX_INTENSITY);
                }
            }
        }
    }

    false
}

/// `PIXEL_FRACTAL`'s state: one bit per row pinning the column it was born in.
///
/// QMK stores `bool led[MATRIX_ROWS][MID_COL]` and shifts each row left by one
/// entry per tick, copying the value out to the mirrored column on the other
/// side. A `u32` per row is the same thing with the mirroring done by bit
/// position, which is what makes the pattern march outwards instead of
/// scrolling.
#[derive(Clone, Copy, Debug, Default)]
pub struct FractalBits {
    rows: [u32; 6],
}

impl FractalBits {
    /// `MID_COL`, QMK's `MATRIX_COLS / 2`, floored at 1.
    const MID_COL: usize = 2;
    const ROWS: usize = 6;

    pub const fn new() -> Self {
        Self { rows: [0; 6] }
    }

    /// Whether row `row`'s newest column is lit.
    #[inline]
    fn head(&self, row: usize) -> bool {
        self.rows[row] & 1 != 0
    }

    /// Whether the entry `l` places from the left edge is lit.
    #[inline]
    fn lit(&self, row: usize, l: usize) -> bool {
        self.rows[row] >> l & 1 != 0
    }

    fn set_head(&mut self, row: usize, lit: bool) {
        if lit {
            self.rows[row] |= 1;
        } else {
            self.rows[row] &= !1;
        }
    }

    fn shift_in(&mut self, row: usize, lit: bool) {
        self.rows[row] = (self.rows[row] << 1) | lit as u32;
    }

    fn clear(&mut self) {
        self.rows = [0; 6];
    }
}

/// `PIXEL_FRACTAL`: a fractal pattern growing outwards from the middle columns.
///
/// Every tick shifts each row one column further from the centre and copies the
/// value to the mirrored column, so a cell lights on both sides at once. A new
/// random head is drawn only when the timer fires.
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

    if ctx.time <= *wait_timer {
        return false;
    }

    let lit = hsv_to_rgb(ctx.config.hsv);
    for row in 0..ROWS {
        // Columns `l` and `COLS - 1 - l` mirror each other.
        for l in 0..MID_COL - 1 {
            let on = bits.lit(row, l);
            let value = if on { lit } else { Rgb::BLACK };
            paint_cell(ctx, params, frame, row, l, value);
            paint_cell(ctx, params, frame, row, COLS - 1 - l, value);
            // Shift this row along, pulling in the next column's bit.
            let next = bits.lit(row, l + 1);
            bits.rows[row] = (bits.rows[row] & !(1 << l)) | ((next as u32) << l);
        }

        // The middle pair shares the head bit.
        let on = bits.head(row);
        let value = if on { lit } else { Rgb::BLACK };
        paint_cell(ctx, params, frame, row, MID_COL - 1, value);
        paint_cell(ctx, params, frame, row, COLS - MID_COL, value);

        // A new random head: `(random8() & 3) ? false : true`, so one in four.
        bits.set_head(row, rng.next8() & 3 == 0);
    }

    *wait_timer = ctx.time.wrapping_add(pixel_fractal_interval(ctx.config.speed));
    false
}

/// `PIXEL_FRACTAL`'s tick interval in milliseconds, from the configured speed.
pub fn pixel_fractal_interval(speed: u8) -> u32 {
    3000 / scale16by8(qadd8(speed, 16) as u16, 16) as u32
}

/// Light the LED at electrical `(row, col)`, if the board has one there.
fn paint_cell<const LED_COUNT: usize, F: LedFrame>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
    row: usize,
    col: usize,
    rgb: Rgb,
) {
    let mut leds = [0u8; 8];
    let count = ctx.leds.leds_at(row as u8, col as u8, &mut leds);
    for &led in &leds[..count] {
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
        // `500 / scale16by8(qadd8(speed,16),16)`: the divisor grows with speed, so
        // a higher speed is a shorter wait.
        assert!(pixel_rain_interval(255) < pixel_rain_interval(0));
        // `qadd8(speed, 16)` is never zero, so the interval is always defined.
        assert!(pixel_rain_interval(0) > 0);
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

        // Armed at 1000: a tick that has not passed it repaints nothing.
        let mut wait = 1000u32;
        pixel_rain(&ctx_at(999), &run, &mut rng, &mut wait, &mut frame);
        assert_eq!(wait, 1000, "still waiting");

        // Crossing it arms the next repaint one interval further out.
        pixel_rain(&ctx_at(1001), &run, &mut rng, &mut wait, &mut frame);
        assert_eq!(wait, 1001 + pixel_rain_interval(128), "rearmed after a repaint");
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
        use crate::rgb::led_config::{LedConfig, LedPoint};

        let leds = Box::leak(Box::new(LedConfig::from_matrix(
            [LedPoint::new(0, 0); 2],
            [(0, 0), (1, 1)],
        )));
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
        typing_heatmap_record(&mut frame_buf, 0, 0);
        assert!(frame_buf.get(0, 0) > 0, "the press heated the cell");

        // The first frame of the effect clears it.
        let params = EffectParams { iter: 0, flags: 0xFF, init: true };
        let mut frame = ArrayFrame::<2>::new();
        typing_heatmap(&ctx, &params, &mut frame_buf, &mut frame);

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
            digital_rain(&ctx, &params, &mut frame_buf, &mut rng, &mut drop, &mut frame_buf_out);

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
