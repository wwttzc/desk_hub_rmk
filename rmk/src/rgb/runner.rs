//! The generic effect runners and the batched-iteration contract they share.
//!
//! Port of QMK's `effect_params_t`, `rgb_matrix_get_limits`, and the six
//! `effect_runner_*.h` helpers. Every effect in QMK is either one of these
//! runners plus a small HSV function, or a hand-written function with the same
//! shape, so this module is what makes the effect ports mechanical.
//!
//! # Batched iteration
//!
//! QMK does not render every LED in one call. `RGB_MATRIX_LED_PROCESS_LIMIT`
//! caps how many LEDs an effect touches per `rgb_matrix_task` invocation, so a
//! keyboard with many LEDs still services USB between batches. The engine calls
//! an effect repeatedly with an incrementing `iter` until the effect reports it
//! reached the end.
//!
//! Two consequences that shape every effect function:
//!
//! - **A batch is a slice of the chain.** `iter` selects `[led_min, led_max)`;
//!   the effect must restrict itself to that range and touch nothing outside it.
//!   [`EffectParams::limits`] computes it, mirroring `RGB_MATRIX_USE_LIMITS`.
//! - **An effect may be called mid-frame.** State that must be reset per frame
//!   cannot simply be a local, and one-shot setup is gated on
//!   [`EffectParams::init`], which the engine sets when the effect or the
//!   enabled state changed.
//!
//! The return value is QMK's `rgb_matrix_check_finished_leds`: `true` means
//! *more batches remain*, not "done".

use super::fx::color::{hsv_to_rgb, Hsv, Rgb};
use super::fx::math8::{qadd8, scale8, scale16by8, sqrt16};
use super::fx::trig8::{cos8, sin8};
use super::hit_tracker::HitTracker;
use super::led_config::{has_any_flags, LedConfig, LedPoint, DEFAULT_CENTER};
use super::state::RgbConfig;

/// `effect_params_t`: how this invocation of an effect should behave.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct EffectParams {
    /// Which batch of the frame this call renders. Zero-based.
    pub iter: u8,
    /// Only LEDs whose flags intersect this are drawn (`params->flags`).
    pub flags: u8,
    /// True when the effect or the enabled state just changed, so per-frame
    /// state should be reset. QMK computes it as
    /// `(effect != rgb_last_effect) || (enable != rgb_last_enable)`.
    pub init: bool,
}

/// `rgb_matrix_get_limits`: the `[min, max)` LED range for batch `iter`.
///
/// `process_limit` is `RGB_MATRIX_LED_PROCESS_LIMIT`. QMK only batches when the
/// limit is both non-zero and smaller than the chain; otherwise every batch
/// covers the whole chain. The arithmetic mirrors the C, including its byte
/// width, so a very large `iter` wraps the same way rather than panicking.
#[inline]
pub fn limits(iter: u8, process_limit: usize, led_count: usize) -> (u8, u8) {
    if process_limit > 0 && process_limit < led_count {
        let min = (process_limit as u8).wrapping_mul(iter);
        let mut max = min.wrapping_add(process_limit as u8);
        if (max as usize) > led_count || max < min {
            max = led_count as u8;
        }
        (min, max)
    } else {
        (0, led_count as u8)
    }
}

impl EffectParams {
    /// `RGB_MATRIX_USE_LIMITS(led_min, led_max)`.
    #[inline]
    pub fn limits(&self, process_limit: usize, led_count: usize) -> (u8, u8) {
        limits(self.iter, process_limit, led_count)
    }
}

/// Where an effect writes its result.
///
/// QMK calls `rgb_matrix_set_color`, which maps the index through the split
/// half before handing it to the driver. On a unibody board that mapping is the
/// identity, so the trait takes the final index; a split build would wrap a
/// swapping implementation instead.
pub trait LedFrame {
    /// Set one LED's colour. `index` is in `0..LED_COUNT`.
    fn set_color(&mut self, index: usize, rgb: Rgb);
}

/// An in-memory frame, for tests and for drivers that want a staging buffer.
///
/// It is also a usable [`LedDriver`](crate::rgb::engine::LedDriver): flushing
/// does nothing, because the pixels already live in `self`. A test that wants to
/// observe flushes should wrap it in its own driver instead.
#[derive(Clone, Debug)]
pub struct ArrayFrame<const LED_COUNT: usize> {
    pub pixels: [Rgb; LED_COUNT],
}

impl<const LED_COUNT: usize> Default for ArrayFrame<LED_COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const LED_COUNT: usize> ArrayFrame<LED_COUNT> {
    pub const fn new() -> Self {
        Self {
            pixels: [Rgb::BLACK; LED_COUNT],
        }
    }

    /// Fill every LED with one colour, `rgb_matrix_set_color_all`.
    pub fn fill(&mut self, rgb: Rgb) {
        self.pixels = [rgb; LED_COUNT];
    }
}

impl<const LED_COUNT: usize> LedFrame for ArrayFrame<LED_COUNT> {
    #[inline]
    fn set_color(&mut self, index: usize, rgb: Rgb) {
        self.pixels[index] = rgb;
    }
}

impl<const LED_COUNT: usize> FrameAll for ArrayFrame<LED_COUNT> {
    fn set_all(&mut self, rgb: Rgb) {
        self.fill(rgb);
    }
}

impl<const LED_COUNT: usize> crate::rgb::engine::LedDriver for ArrayFrame<LED_COUNT> {
    async fn flush(&mut self) {
        // The pixels are already in memory; a real driver would shift them out.
    }
}

/// Everything a runner reads: the board's LED table, the run-time config, the
/// frame clock and the batch limit.
///
/// QMK reaches these through globals (`g_led_config`, `rgb_matrix_config`,
/// `g_rgb_timer`). Passing them explicitly keeps the effects testable and lets
/// the same code run against a mock clock on the host.
pub struct RenderContext<'a, const LED_COUNT: usize> {
    pub leds: &'a LedConfig<LED_COUNT>,
    pub config: &'a RgbConfig,
    /// Centroid the distance-based effects measure from,
    /// `k_rgb_matrix_center`. QMK's default is `{112, 32}`.
    pub center: LedPoint,
    /// Frame clock in milliseconds, `g_rgb_timer`. Truncated to 16 bits at the
    /// call sites exactly as the C does.
    pub time: u32,
    /// `RGB_MATRIX_LED_PROCESS_LIMIT`.
    pub process_limit: usize,
}

impl<const LED_COUNT: usize> RenderContext<'_, LED_COUNT> {
    /// The default centroid, matching QMK's `k_rgb_matrix_center`.
    pub const DEFAULT_CENTER: LedPoint = DEFAULT_CENTER;

    /// `rgb_matrix_check_finished_leds`: true when this batch stopped short of
    /// the end, so the engine must call the effect again.
    #[inline]
    pub const fn more_batches(&self, led_max: u8) -> bool {
        (led_max as usize) < LED_COUNT
    }
}

/// `RGB_MATRIX_TEST_LED_FLAGS()`: an LED whose flags do not intersect the
/// active filter is left alone this frame.
#[inline]
fn led_selected<const LED_COUNT: usize>(ctx: &RenderContext<'_, LED_COUNT>, params: &EffectParams, i: u8) -> bool {
    has_any_flags(ctx.leds.flags(i as usize), params.flags)
}

/// Write one LED from an HSV result, as the runners all end by doing.
#[inline]
fn put<F: LedFrame>(frame: &mut F, i: u8, hsv: Hsv) {
    frame.set_color(i as usize, hsv_to_rgb(hsv));
}

/// `effect_runner_i`: `time = scale16by8(timer, speed/4 + 1)`, then a per-index
/// HSV function.
///
/// Drives `BREATHING`, `CYCLE_ALL`, `RAINDROPS` and the other index-driven
/// effects.
pub fn effect_runner_i<const LED_COUNT: usize, F, E>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
    mut effect_func: E,
) -> bool
where
    F: LedFrame,
    E: FnMut(Hsv, u8, u8) -> Hsv,
{
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let time = scale16by8(ctx.time as u16, qadd8(ctx.config.speed / 4, 1)) as u8;

    for i in led_min..led_max {
        if !led_selected(ctx, params, i) {
            continue;
        }
        let hsv = effect_func(ctx.config.hsv, i, time);
        put(frame, i, hsv);
    }

    ctx.more_batches(led_max)
}

/// `effect_runner_dx_dy`: same as [`effect_runner_i`] but the effect also gets
/// the LED's offset from the keyboard centre.
pub fn effect_runner_dx_dy<const LED_COUNT: usize, F, E>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
    mut effect_func: E,
) -> bool
where
    F: LedFrame,
    E: FnMut(Hsv, i16, i16, u8) -> Hsv,
{
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let time = scale16by8(ctx.time as u16, ctx.config.speed / 2) as u8;

    for i in led_min..led_max {
        if !led_selected(ctx, params, i) {
            continue;
        }
        let (dx, dy) = offset(ctx, i);
        let hsv = effect_func(ctx.config.hsv, dx, dy, time);
        put(frame, i, hsv);
    }

    ctx.more_batches(led_max)
}

/// `effect_runner_dx_dy_dist`: [`effect_runner_dx_dy`] plus the Euclidean
/// distance from the centre.
pub fn effect_runner_dx_dy_dist<const LED_COUNT: usize, F, E>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
    mut effect_func: E,
) -> bool
where
    F: LedFrame,
    E: FnMut(Hsv, i16, i16, u8, u8) -> Hsv,
{
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let time = scale16by8(ctx.time as u16, ctx.config.speed / 2) as u8;

    for i in led_min..led_max {
        if !led_selected(ctx, params, i) {
            continue;
        }
        let (dx, dy) = offset(ctx, i);
        let dist = distance(dx, dy);
        let hsv = effect_func(ctx.config.hsv, dx, dy, dist, time);
        put(frame, i, hsv);
    }

    ctx.more_batches(led_max)
}

/// `effect_runner_sin_cos_i`: the effect gets a shared sine/cosine pair, so
/// every LED animates on the same beat but at its own index.
///
/// `time` is 16-bit here (unlike the other runners) because it feeds `sin8`/
/// `cos8`, which take a byte, and the runner passes the wide value through.
pub fn effect_runner_sin_cos_i<const LED_COUNT: usize, F, E>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    frame: &mut F,
    mut effect_func: E,
) -> bool
where
    F: LedFrame,
    E: FnMut(Hsv, i8, i8, u8, u8) -> Hsv,
{
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let time = scale16by8(ctx.time as u16, ctx.config.speed / 4);

    let cos_value = (cos8(time as u8) as i16 - 128) as i8;
    let sin_value = (sin8(time as u8) as i16 - 128) as i8;

    for i in led_min..led_max {
        if !led_selected(ctx, params, i) {
            continue;
        }
        let hsv = effect_func(ctx.config.hsv, cos_value, sin_value, i, time as u8);
        put(frame, i, hsv);
    }

    ctx.more_batches(led_max)
}

/// `effect_runner_reactive`: the effect gets how long ago *this* LED was hit.
///
/// The "no recent hit" value is `max_tick`, which makes the LED sit at the far
/// end of the effect's decay rather than jumping to a default.
pub fn effect_runner_reactive<const LED_COUNT: usize, F, E>(
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    hits: &HitTracker,
    frame: &mut F,
    mut effect_func: E,
) -> bool
where
    F: LedFrame,
    E: FnMut(Hsv, u16) -> Hsv,
{
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let max_tick = 65535 / qadd8(ctx.config.speed, 1) as u16;

    for i in led_min..led_max {
        if !led_selected(ctx, params, i) {
            continue;
        }
        let tick = hits.most_recent_tick(i, max_tick);
        let offset = scale16by8(tick, qadd8(ctx.config.speed, 1));
        let hsv = effect_func(ctx.config.hsv, offset);
        put(frame, i, hsv);
    }

    ctx.more_batches(led_max)
}

/// `effect_runner_reactive_splash`: the effect gets every remembered hit in
/// turn, accumulating into one LED.
///
/// `start` skips the oldest hits, which is how QMK's splash effects avoid
/// re-accumulating a hit they have already faded out. The final value is
/// scaled by the configured brightness, so a splash never exceeds the global
/// level.
pub fn effect_runner_reactive_splash<const LED_COUNT: usize, F, E>(
    start: u8,
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    hits: &HitTracker,
    frame: &mut F,
    mut effect_func: E,
) -> bool
where
    F: LedFrame,
    E: FnMut(Hsv, i16, i16, u8, u16) -> Hsv,
{
    let (led_min, led_max) = params.limits(ctx.process_limit, LED_COUNT);
    let count = hits.count();

    for i in led_min..led_max {
        if !led_selected(ctx, params, i) {
            continue;
        }
        // Start black: the splashes add up, they do not overwrite.
        let mut hsv = ctx.config.hsv;
        hsv.v = 0;

        let point = ctx.leds.point(i as usize);
        for j in start..count {
            let hit = match hits.hit(j) {
                Some(hit) => hit,
                None => break,
            };
            let dx = point.x as i16 - hit.x as i16;
            let dy = point.y as i16 - hit.y as i16;
            let dist = distance(dx, dy);
            let tick = scale16by8(hit.tick, qadd8(ctx.config.speed, 1));
            hsv = effect_func(hsv, dx, dy, dist, tick);
        }

        hsv.v = scale8(hsv.v, ctx.config.hsv.v);
        put(frame, i, hsv);
    }

    ctx.more_batches(led_max)
}

/// `rgb_matrix_none`: the off state clears the frame once, then does nothing.
///
/// It returns `false` (no more batches) so the engine flushes immediately.
pub fn rgb_matrix_none<F: LedFrame + FrameAll>(params: &EffectParams, frame: &mut F) -> bool {
    if !params.init {
        return false;
    }
    frame.set_all(Rgb::BLACK);
    false
}

/// `rgb_matrix_set_color_all`, needed by [`rgb_matrix_none`].
pub trait FrameAll {
    fn set_all(&mut self, rgb: Rgb);
}

/// `point[i] - center`, in the signed form the C promotes to.
#[inline]
fn offset<const LED_COUNT: usize>(ctx: &RenderContext<'_, LED_COUNT>, i: u8) -> (i16, i16) {
    let point = ctx.leds.point(i as usize);
    (
        point.x as i16 - ctx.center.x as i16,
        point.y as i16 - ctx.center.y as i16,
    )
}

/// `sqrt16(dx * dx + dy * dy)`, widening before the sum as the C does.
#[inline]
fn distance(dx: i16, dy: i16) -> u8 {
    sqrt16((dx as i32 * dx as i32 + dy as i32 * dy as i32) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::fx::color::Hsv;
    use crate::rgb::led_config::{LED_FLAG_ALL, LED_FLAG_KEYLIGHT, LED_FLAG_UNDERGLOW};
    use crate::rgb::state::{RgbConfig, DEFAULT_FLAGS};

    const LEDS: usize = 8;

    /// Eight LEDs in a row at x = 0, 32, 64, ... 224, all keylights.
    fn leds() -> LedConfig<LEDS> {
        let mut points = [LedPoint::new(0, 32); LEDS];
        let mut matrix = [(0u8, 0u8); LEDS];
        for i in 0..LEDS {
            points[i] = LedPoint::new((i as u8) * 32, 32);
            matrix[i] = (0, i as u8);
        }
        LedConfig::from_matrix(points, matrix)
    }

    /// The same chain, but the last two LEDs are underglow.
    fn leds_with_underglow() -> LedConfig<LEDS> {
        let mut flags = [LED_FLAG_KEYLIGHT; LEDS];
        flags[LEDS - 2] = LED_FLAG_UNDERGLOW;
        flags[LEDS - 1] = LED_FLAG_UNDERGLOW;
        let base = leds();
        let mut points = [LedPoint::new(0, 32); LEDS];
        let mut matrix = [(0u8, 0u8); LEDS];
        for i in 0..LEDS {
            points[i] = base.point(i);
            matrix[i] = base.matrix_pos(i).unwrap();
        }
        LedConfig::new(points, flags, matrix)
    }

    fn config() -> RgbConfig {
        RgbConfig::with_defaults(true, 0, 255, 127, DEFAULT_FLAGS, 255)
    }

    /// A context whose timer is non-zero, so `time` is not trivially zero.
    fn ctx<'a>(leds: &'a LedConfig<LEDS>, config: &'a RgbConfig, process_limit: usize) -> RenderContext<'a, LEDS> {
        RenderContext {
            leds,
            config,
            center: RenderContext::<LEDS>::DEFAULT_CENTER,
            time: 1234,
            process_limit,
        }
    }

    #[test]
    fn limits_batch_only_when_the_limit_is_smaller_than_the_chain() {
        // process_limit >= led_count: one batch covers everything.
        assert_eq!(limits(0, 8, 8), (0, 8));
        assert_eq!(limits(0, 20, 8), (0, 8));
        assert_eq!(limits(5, 20, 8), (0, 8), "iter is ignored");
        // process_limit == 0 also disables batching.
        assert_eq!(limits(3, 0, 8), (0, 8));
    }

    #[test]
    fn limits_slice_the_chain_when_batching() {
        // 8 LEDs, limit 3: batches of 3, 3, 2.
        assert_eq!(limits(0, 3, 8), (0, 3));
        assert_eq!(limits(1, 3, 8), (3, 6));
        assert_eq!(limits(2, 3, 8), (6, 8), "the last batch is clamped");
        // Past the end the range inverts: min 9 > max 8. The C display loop
        // `for (i = min; i < max; i++)` then does nothing, which is how QMK
        // signals "this batch is empty". Not a bug to be tidied away.
        assert_eq!(limits(3, 3, 8), (9, 8));
    }

    #[test]
    fn limits_wrap_like_the_byte_arithmetic_in_c() {
        // `min` and `max` are uint8_t in QMK; a large iter must not panic.
        let (min, max) = limits(200, 3, 8);
        assert_eq!(min, (3u16 * 200u16 % 256) as u8);
        // Wrapped min > max means the range is empty, and max is clamped.
        assert!(max >= min || max == 8);
        assert!(max <= 8);
    }

    #[test]
    fn the_default_limit_batches_a_long_chain() {
        // QMK's default is (LED_COUNT + 4) / 5, which `default_process_limit`
        // spells as `div_ceil`.
        let process_limit = LEDS.div_ceil(5); // 2
        assert_eq!(limits(0, process_limit, LEDS), (0, 2));
        assert_eq!(limits(3, process_limit, LEDS), (6, 8));
        assert_eq!(limits(4, process_limit, LEDS), (8, 8));
    }
    /// The core batching contract: a full frame needs several calls, each
    /// touching only its own slice, and the last one reports completion.
    #[test]
    fn a_batched_frame_covers_every_led_exactly_once() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config, 3);
        let mut frame = ArrayFrame::<LEDS>::new();
        let mut touched = [0u8; LEDS];

        let mut iter = 0;
        let mut calls = 0;
        loop {
            let params = EffectParams {
                iter,
                flags: LED_FLAG_ALL,
                init: iter == 0,
            };
            let (min, max) = params.limits(ctx.process_limit, LEDS);
            for i in min..max {
                touched[i as usize] += 1;
            }

            let more = effect_runner_i(&ctx, &params, &mut frame, |hsv, _, _| hsv);
            calls += 1;
            if !more {
                break;
            }
            iter += 1;
            assert!(calls < 10, "batching must terminate");
        }

        assert_eq!(calls, 3, "8 LEDs at 3 per batch");
        assert_eq!(touched, [1; LEDS], "every LED was in exactly one batch");
    }

    #[test]
    fn unbounded_batching_finishes_in_one_call() {
        let leds = leds();
        let config = config();
        // process_limit >= LEDS, so one call covers the chain.
        let ctx = ctx(&leds, &config, LEDS);
        let mut frame = ArrayFrame::<LEDS>::new();
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        assert!(
            !effect_runner_i(&ctx, &params, &mut frame, |hsv, _, _| hsv),
            "a single batch that reaches the end reports completion"
        );
    }

    #[test]
    fn the_flag_filter_skips_unselected_leds() {
        let leds = leds_with_underglow();
        let config = config();
        let ctx = ctx(&leds, &config, LEDS);
        let mut frame = ArrayFrame::<LEDS>::new();
        frame.fill(Rgb::WHITE);

        // Only draw the underglow LEDs.
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_UNDERGLOW,
            init: true,
        };
        effect_runner_i(&ctx, &params, &mut frame, |_, _, _| Hsv::new(0, 255, 255));

        for i in 0..LEDS {
            let expect_drawn = i >= LEDS - 2;
            let was_drawn = frame.pixels[i] == Rgb::RED;
            assert_eq!(was_drawn, expect_drawn, "LED {i}");
        }
    }

    #[test]
    fn runner_i_computes_time_from_the_speed() {
        let leds = leds();
        let mut config = config();
        config.set_speed(255);
        let ctx = ctx(&leds, &config, LEDS);
        let mut frame = ArrayFrame::<LEDS>::new();

        // Capture the time the effect is handed.
        let mut seen = None;
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        effect_runner_i(&ctx, &params, &mut frame, |_, _, time| {
            seen = Some(time);
            Hsv::default()
        });

        let expected = scale16by8(ctx.time as u16, qadd8(255 / 4, 1)) as u8;
        assert_eq!(seen, Some(expected));
    }

    #[test]
    fn runner_dx_dy_measures_from_the_center() {
        use core::cell::Cell;

        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config, LEDS);
        let mut frame = ArrayFrame::<LEDS>::new();

        // `dx_dy` effects do not receive the index, but the runner visits LEDs
        // in ascending order, so a counter recovers the mapping.
        let idx = Cell::new(0usize);
        let seen = Cell::new([(0i16, 0i16); LEDS]);
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        effect_runner_dx_dy(&ctx, &params, &mut frame, |_, dx, dy, _| {
            let mut s = seen.get();
            s[idx.get()] = (dx, dy);
            seen.set(s);
            idx.set(idx.get() + 1);
            Hsv::default()
        });

        let center = ctx.center;
        assert_eq!(idx.get(), LEDS, "every LED was visited");
        for (i, (dx, dy)) in seen.get().iter().enumerate() {
            assert_eq!(
                (*dx, *dy),
                (
                    leds.point(i).x as i16 - center.x as i16,
                    leds.point(i).y as i16 - center.y as i16,
                ),
                "offset for LED {i}"
            );
        }
        // Sanity-check the ends so the assertion above cannot pass vacuously.
        assert_eq!(seen.get()[0].0, -112, "leftmost");
        assert_eq!(seen.get()[LEDS - 1].0, 112, "rightmost");
    }

    #[test]
    fn runner_dx_dy_dist_reports_euclidean_distance() {
        // A 3-4-5 triangle from the centre, so the expected distance is exact.
        let leds = LedConfig::from_matrix([LedPoint::new(112 + 3, 32 + 4)], [(0, 0)]);
        let config = config();
        let ctx = RenderContext::<1> {
            leds: &leds,
            config: &config,
            center: LedPoint::new(112, 32),
            time: 0,
            process_limit: 1,
        };
        let mut frame = ArrayFrame::<1>::new();

        let mut dist = None;
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        effect_runner_dx_dy_dist(&ctx, &params, &mut frame, |_, _, _, d, _| {
            dist = Some(d);
            Hsv::default()
        });
        assert_eq!(dist, Some(5));
    }

    /// QMK's own naming trap: `sin_cos_i_f` is declared as
    /// `(hsv, int8_t sin, int8_t cos, ...)`, but every call site passes
    /// `(hsv, cos_value, sin_value, ...)` — i.e. the first argument is the
    /// **cosine**. Effects are written against the call order, so the runner
    /// passes cosine first and this test pins that.
    #[test]
    fn runner_sin_cos_i_passes_cosine_first() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config, LEDS);
        let mut frame = ArrayFrame::<LEDS>::new();

        let mut pair = None;
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        effect_runner_sin_cos_i(&ctx, &params, &mut frame, |_, first, second, _, _| {
            pair = Some((first, second));
            Hsv::default()
        });

        let (first, second) = pair.expect("the effect ran");
        let time = scale16by8(ctx.time as u16, config.speed / 4) as u8;

        assert_eq!(first, (cos8(time) as i16 - 128) as i8, "first argument is cosine");
        assert_eq!(second, (sin8(time) as i16 - 128) as i8, "second is sine");
        // Both are byte-derived and centred: -128..=127.
        assert!((-128..=127).contains(&first));
        assert!((-128..=127).contains(&second));
        assert_ne!(first, second, "cosine and sine differ away from the axes");
    }

    #[test]
    fn runner_reactive_gives_the_most_recent_hit_offsets() {
        let leds = leds();
        let config = config();
        let ctx = ctx(&leds, &config, LEDS);
        let mut hits = HitTracker::new();
        hits.record(&leds, 0, 2);
        hits.age(10);
        hits.record(&leds, 0, 5);

        let mut offsets = [0u16; LEDS];
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        let mut idx = 0usize;
        let mut frame = ArrayFrame::<LEDS>::new();
        effect_runner_reactive(&ctx, &params, &hits, &mut frame, |_, offset| {
            offsets[idx] = offset;
            idx += 1;
            Hsv::default()
        });
        assert_eq!(idx, LEDS, "every LED was visited");

        let speed = qadd8(config.speed, 1) as u16;
        let max_tick = 65535 / speed;
        // LED 2 was hit and aged to 10; LED 5 was hit most recently (tick 0).
        assert_eq!(offsets[2], scale16by8(10, qadd8(config.speed, 1)));
        assert_eq!(offsets[5], scale16by8(0, qadd8(config.speed, 1)));
        // An untouched LED gets max_tick, which puts it past the decay end.
        assert_eq!(offsets[0], scale16by8(max_tick, qadd8(config.speed, 1)));
    }

    #[test]
    fn splash_accumulates_every_hit_and_scales_by_brightness() {
        let leds = LedConfig::from_matrix([LedPoint::new(112, 32)], [(0, 0)]);
        let mut config = config();
        config.set_hsv(0, 255, 128, 255);
        let ctx = RenderContext::<1> {
            leds: &leds,
            config: &config,
            center: LedPoint::new(112, 32),
            time: 0,
            process_limit: 1,
        };
        let mut hits = HitTracker::new();
        hits.record(&leds, 0, 0);
        hits.record(&leds, 0, 0);

        let mut calls = 0;
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        let mut frame = ArrayFrame::<1>::new();
        effect_runner_reactive_splash(0, &ctx, &params, &hits, &mut frame, |mut hsv, _, _, _, _| {
            calls += 1;
            // Accumulate brightness so the final scale is observable.
            hsv.v = qadd8(hsv.v, 255);
            hsv
        });

        assert_eq!(calls, 2, "both hits were accumulated into the one LED");
        // Brightness was capped at 255 by the effect, then scaled by v = 128.
        assert_eq!(frame.pixels[0], hsv_to_rgb(Hsv::new(0, 255, scale8(255, 128))));
    }

    #[test]
    fn splash_start_skips_the_oldest_hits() {
        let leds = LedConfig::from_matrix([LedPoint::new(112, 32)], [(0, 0)]);
        let config = config();
        let ctx = RenderContext::<1> {
            leds: &leds,
            config: &config,
            center: LedPoint::new(112, 32),
            time: 0,
            process_limit: 1,
        };
        let mut hits = HitTracker::new();
        hits.record(&leds, 0, 0);
        hits.record(&leds, 0, 0);
        hits.record(&leds, 0, 0);

        let mut calls = 0;
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: true,
        };
        let mut frame = ArrayFrame::<1>::new();
        effect_runner_reactive_splash(2, &ctx, &params, &hits, &mut frame, |hsv, _, _, _, _| {
            calls += 1;
            hsv
        });
        assert_eq!(calls, 1, "only hits from index 2 onward were accumulated");

        // And without `start`, all three are accumulated.
        let mut all = 0;
        effect_runner_reactive_splash(0, &ctx, &params, &hits, &mut frame, |hsv, _, _, _, _| {
            all += 1;
            hsv
        });
        assert_eq!(all, 3);
    }

    #[test]
    fn none_clears_once_on_init_and_reports_completion() {
        let mut config = config();
        config.disable();
        let mut frame = ArrayFrame::<LEDS>::new();
        frame.fill(Rgb::WHITE);

        // Without init it must not touch the frame.
        let params = EffectParams {
            iter: 0,
            flags: LED_FLAG_ALL,
            init: false,
        };
        assert!(!rgb_matrix_none(&params, &mut frame));
        assert_eq!(frame.pixels[0], Rgb::WHITE, "left alone");

        // With init it clears and still reports completion.
        let params = EffectParams { init: true, ..params };
        assert!(!rgb_matrix_none(&params, &mut frame));
        assert_eq!(frame.pixels, [Rgb::BLACK; LEDS]);
    }
}
