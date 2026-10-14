//! The RGB engine: the task that turns the state, the effect table and the
//! elapsed time into frames of pixels, and hands them to a driver.
//!
//! This is the integration point. Everything below it is a pure function of its
//! inputs; everything above it is a board-specific LED driver. Port of QMK's
//! `rgb_matrix_task` and its four-state machine.
//!
//! # Why a state machine
//!
//! QMK renders a frame in batches so a long chain does not monopolise the CPU:
//! one `rgb_matrix_task` call advances the machine by at most one batch, and
//! between calls the firmware services USB. The states are
//!
//! - `Starting`: latch the frame clock and the recent-hit snapshot, pick the
//!   effect to render (which is `NONE` when disabled), reset `iter`.
//! - `Rendering`: render one batch, advancing `iter`.
//! - `Flushing`: push the finished frame to the LEDs.
//! - `Syncing`: wait out `led_flush_limit` before starting the next frame.
//!
//! [`RgbEngine::tick`] drives it one step. A caller that owns a timer drives it
//! on a fixed cadence; the tests drive it by hand, which is what makes the
//! batching contract testable without hardware.

use super::fx::random8::Rng;
use super::hit_tracker::HitTracker;
use super::led_config::LedConfig;
use super::registry::{self, EFFECT_OFF};
use super::runner::{EffectParams, FrameAll, LedFrame, RenderContext};
use super::state::RgbConfig;

/// `RGB_MATRIX_LED_FLUSH_LIMIT`: the minimum milliseconds between frames.
///
/// 16 ms is QMK's default, i.e. a 60 fps ceiling. QMK expresses the wait by
/// comparing elapsed time against this; we keep the value and compare the same
/// way so the frame rate matches.
pub const LED_FLUSH_LIMIT_MS: u32 = 16;

/// `RGB_MATRIX_LED_PROCESS_LIMIT`: LEDs per batch, QMK's `(LED_COUNT + 4) / 5`.
///
/// Computed rather than fixed so a 19-LED board batches at 4 and a hypothetical
/// 100-LED board at 20, both of which keep one batch short enough to leave room
/// for other work between ticks.
pub const fn default_process_limit(led_count: usize) -> usize {
    led_count.div_ceil(5)
}

/// The four task states, QMK's `rgb_task_states`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Starting,
    Rendering,
    Flushing,
    Syncing,
}

/// A sink for finished frames.
///
/// Implemented by the board for a real LED chain, and by
/// [`VecFrame`](super::runner::ArrayFrame) in tests. Kept separate from
/// [`LedFrame`] so the engine can hold one object that both accumulates a frame
/// and flushes it, while effects only ever see the accumulate half.
pub trait LedDriver {
    /// Push the accumulated frame out to the LEDs.
    ///
    /// Async because the real driver is: `esp-hal-smartled`'s
    /// `SmartLedsWriteAsync` yields while the RMT peripheral shifts the bits,
    /// which keeps a 19-LED frame from blocking the USB task for the ~570 us the
    /// wire takes.
    fn flush(&mut self) -> impl core::future::Future<Output = ()>;
}

/// What the engine needs from a frame buffer: the effect write path, the
/// all-LEDs fill that `RGB_MATRIX_NONE` uses, and a way to flush.
pub trait FrameBuffer: LedFrame + FrameAll + LedDriver {}

impl<T: LedFrame + FrameAll + LedDriver> FrameBuffer for T {}

/// The per-frame scratch the engine keeps between ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameContext {
    /// `effect_params_t`.
    params: EffectParams,
    /// Which effect the current frame renders, resolved in `Starting`.
    effect: u16,
    /// `rgb_last_effect`: compared against to compute `params.init`.
    last_effect: u16,
    /// `rgb_last_enable`.
    last_enable: bool,
}

impl Default for FrameContext {
    fn default() -> Self {
        Self {
            params: EffectParams {
                iter: 0,
                flags: super::led_config::LED_FLAG_ALL,
                init: true,
            },
            // `UINT8_MAX` in QMK: a value no real effect has, so the first frame
            // always counts as initialising.
            effect: u16::MAX,
            last_effect: u16::MAX,
            last_enable: false,
        }
    }
}

/// The RGB Matrix task.
///
/// Generic over the LED count and the frame buffer so the whole engine runs on
/// the host against a mock driver.
pub struct RgbEngine<const LED_COUNT: usize, F: FrameBuffer> {
    state: TaskState,
    config: RgbConfig,
    /// `rgb_config_t`'s brightness ceiling, from the board config.
    max_brightness: u8,
    leds: &'static LedConfig<LED_COUNT>,
    /// Recent key hits, the input to the reactive effects.
    hits: HitTracker,
    /// The LCG shared by the random effects.
    rng: Rng,
    /// `RAINDROPS`'s `static uint16_t index`.
    raindrops_index: u16,
    /// `g_direct_mode_colors`: per-LED colours the host set directly.
    direct: super::vialrgb::DirectColors<LED_COUNT>,
    frame: F,
    ctx: FrameContext,
    /// Ledger mirror of `frame.params`, held separately so the effect call does
    /// not borrow the frame mutably twice.
    process_limit: usize,
    /// The frame clock, QMK's `g_rgb_timer`.
    timer_ms: u32,
    /// When the current frame started, for the flush-limit wait.
    frame_started_ms: u32,
}

impl<const LED_COUNT: usize, F: FrameBuffer> RgbEngine<LED_COUNT, F> {
    /// Build an engine around a driver and a board LED table.
    ///
    /// `max_brightness` comes from the configuration and is what the Vial
    /// brightness slider tops out at; `process_limit` is
    /// [`default_process_limit`] unless the config overrides it.
    pub fn new(leds: &'static LedConfig<LED_COUNT>, config: RgbConfig, max_brightness: u8, frame: F) -> Self {
        Self {
            state: TaskState::Syncing,
            config,
            max_brightness,
            leds,
            hits: HitTracker::new(),
            rng: Rng::new(),
            raindrops_index: u16::MAX,
            direct: super::vialrgb::DirectColors::new(),
            frame,
            ctx: FrameContext::default(),
            process_limit: default_process_limit(LED_COUNT),
            timer_ms: 0,
            // Seed the frame clock one flush limit in the past so the first
            // `Syncing` tick releases immediately: the keyboard should light on
            // the first task run, not after a 16 ms wait. QMK lands in the same
            // place because its `rgb_task_state` also starts at `SYNCING` with an
            // already-elapsed sync timer.
            frame_started_ms: 0u32.wrapping_sub(LED_FLUSH_LIMIT_MS),
        }
    }

    /// Override the batch size, from `rgb_matrix.led_process_limit`.
    pub fn with_process_limit(mut self, limit: usize) -> Self {
        self.process_limit = limit;
        self
    }

    /// The run-time configuration, as the Vial protocol reads it back.
    #[inline]
    pub fn config(&self) -> &RgbConfig {
        &self.config
    }

    /// Mutable access, for the Vial setters and the keycode handlers.
    #[inline]
    pub fn config_mut(&mut self) -> &mut RgbConfig {
        &mut self.config
    }

    /// The brightness ceiling, `RGB_MATRIX_MAXIMUM_BRIGHTNESS`.
    #[inline]
    pub const fn max_brightness(&self) -> u8 {
        self.max_brightness
    }

    /// The current task state, for tests.
    #[inline]
    pub const fn state(&self) -> TaskState {
        self.state
    }

    /// Advance the task by one step, without an async context.
    ///
    /// `tick` never actually awaits: the only `await` is the driver's `flush`,
    /// which for every driver in this crate resolves immediately. This wrapper
    /// exists so tests that do not have a runtime can drive the machine, and it
    /// panics rather than hanging if a driver ever does suspend.
    #[cfg(any(test, feature = "std"))]
    pub fn tick_sync(&mut self) -> TaskState {
        fn poll_once<F: core::future::Future>(fut: F) -> Option<F::Output> {
            use core::task::{Context, Poll, Waker};
            fn noop_waker() -> Waker {
                use core::task::{RawWaker, RawWakerVTable};
                const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
                const RAW: RawWaker = RawWaker::new(core::ptr::null(), &VTABLE);
                // Safety: every vtable entry is a no-op and no state is read
                // through the null data pointer.
                unsafe { Waker::from_raw(RAW) }
            }
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            let mut fut = core::pin::pin!(fut);
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(out) => Some(out),
                Poll::Pending => None,
            }
        }

        poll_once(self.tick()).expect("the RGB driver suspended; drive it with `tick().await` instead")
    }

    /// The frame buffer, for tests and for the flush-side driver.
    #[inline]
    pub fn frame(&self) -> &F {
        &self.frame
    }

    /// The host-driven pixel buffer, for `VIALRGB_DIRECT_FASTSET`.
    #[inline]
    pub fn direct_colors(&self) -> &super::vialrgb::DirectColors<LED_COUNT> {
        &self.direct
    }

    /// Mutable access, so the protocol layer can write pixels.
    #[inline]
    pub fn direct_colors_mut(&mut self) -> &mut super::vialrgb::DirectColors<LED_COUNT> {
        &mut self.direct
    }

    /// Feed a key event to the hit tracker.
    ///
    /// This is QMK's `rgb_matrix_handle_key_event`. `pressed` selects which
    /// transitions count: `RGB_MATRIX_KEYRELEASES` is off by default, so a hit
    /// is recorded on key-down.
    pub fn handle_key_event(&mut self, row: u8, col: u8, pressed: bool) {
        if pressed {
            self.hits.record(self.leds, row, col);
        }
    }

    /// Age every remembered hit, QMK's `rgb_task_timers`.
    ///
    /// The caller passes how long since the last call, because the engine does
    /// not own a clock.
    pub fn advance_time(&mut self, now_ms: u32) {
        let delta = now_ms.wrapping_sub(self.timer_ms);
        self.timer_ms = now_ms;
        self.hits.age(delta);
    }

    /// The state the engine needs to persist, as `StorageValue::RgbConfig`.
    #[inline]
    pub fn config_for_storage(&self) -> RgbConfig {
        self.config
    }

    /// Restore configuration read back from storage.
    pub fn restore_config(&mut self, config: RgbConfig) {
        self.config = config;
    }

    /// Advance the task by one step.
    ///
    /// Drives the same four states as `rgb_matrix_task`. Returns the state the
    /// engine moved to, which the tests assert on and a real caller can ignore.
    pub async fn tick(&mut self) -> TaskState {
        match self.state {
            TaskState::Starting => self.start(),
            TaskState::Rendering => self.render(),
            TaskState::Flushing => {
                self.frame.flush().await;
                self.state = TaskState::Syncing;
            }
            TaskState::Syncing => {
                // `rgb_task_sync`: hold a frame back until the flush limit has
                // passed, so a fast caller cannot exceed the configured rate.
                if self.timer_ms.wrapping_sub(self.frame_started_ms) >= LED_FLUSH_LIMIT_MS {
                    self.state = TaskState::Starting;
                }
            }
        }
        self.state
    }

    /// `rgb_task_start`.
    fn start(&mut self) {
        self.ctx.params.iter = 0;
        self.frame_started_ms = self.timer_ms;

        // `rgb_task_start` picks `NONE` when the LEDs are off, so a disabled
        // keyboard clears its frame instead of freezing on the last animation.
        self.ctx.effect = if self.config.enable {
            self.config.mode
        } else {
            EFFECT_OFF
        };

        self.ctx.params.flags = self.config.flags;
        self.state = TaskState::Rendering;
    }

    /// `rgb_task_render`, one batch.
    fn render(&mut self) {
        let effect = self.ctx.effect;

        // `rgb_effect_params.init` is set when the effect or the enabled state
        // changed, which is how an effect knows to reset its one-shot state.
        self.ctx.params.init = (effect != self.ctx.last_effect) || (self.config.enable != self.ctx.last_enable);

        // `DIRECT_MODE` renders the host's own pixels instead of an effect.
        // Handled before the batch machinery: a direct frame is one batch, since
        // there is no per-LED work to spread out.
        if effect == super::vialrgb::DIRECT_MODE {
            self.render_direct();
            self.ctx.params.iter = self.ctx.params.iter.saturating_add(1);
            self.ctx.last_effect = effect;
            self.ctx.last_enable = self.config.enable;
            self.state = TaskState::Flushing;
            return;
        }

        // Destructure so the effect sees disjoint borrows of the engine's
        // fields: the frame is written while the config, LED table, hits and RNG
        // are read. Going through `self` for each would borrow all of `self`
        // twice and force a copy of the frame.
        let Self {
            config,
            leds,
            hits,
            rng,
            raindrops_index,
            frame,
            ctx,
            process_limit,
            timer_ms,
            ..
        } = self;

        let more = registry::dispatch(
            effect,
            &RenderContext {
                leds,
                config,
                center: super::led_config::DEFAULT_CENTER,
                time: *timer_ms,
                process_limit: *process_limit,
            },
            &ctx.params,
            hits,
            rng,
            raindrops_index,
            frame,
        );

        self.ctx.params.iter = self.ctx.params.iter.saturating_add(1);

        if !more {
            self.ctx.last_effect = effect;
            self.ctx.last_enable = self.config.enable;
            self.state = TaskState::Flushing;
        }
    }

    /// Paint the frame from the host's `DIRECT_FASTSET` pixels.
    ///
    /// Every LED is written each frame, because a report carries only nine of
    /// them and the rest must remain what the host set earlier; the engine's own
    /// buffer is the memory of those earlier reports.
    fn render_direct(&mut self) {
        for i in 0..LED_COUNT {
            let (h, s, v) = self.direct.get(i);
            let rgb = super::fx::color::hsv_to_rgb(super::fx::color::Hsv::new(h, s, v));
            self.frame.set_color(i, rgb);
        }
    }
}

/// Whether an effect id can be selected at all.
///
/// `EFFECT_OFF` always can (it means "off"); an extension id is selectable by
/// keycode and by configuration even though Vial cannot name it.
pub fn is_selectable(id: u16) -> bool {
    id == EFFECT_OFF || registry::EffectEntry::by_id(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::fx::color::Rgb;
    use crate::rgb::fx::math8::scale8;
    use crate::rgb::led_config::LedPoint;
    use crate::rgb::registry::EFFECT_MAX;
    use crate::rgb::runner::ArrayFrame;

    const N: usize = 6;

    fn leds() -> &'static LedConfig<N> {
        Box::leak(Box::new(LedConfig::from_matrix(
            [
                LedPoint::new(0, 0),
                LedPoint::new(45, 0),
                LedPoint::new(90, 0),
                LedPoint::new(135, 64),
                LedPoint::new(180, 64),
                LedPoint::new(224, 64),
            ],
            [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)],
        )))
    }

    /// A driver that records how many times it was flushed.
    #[derive(Default)]
    struct MockDriver {
        frame: ArrayFrame<N>,
        flushes: usize,
    }

    impl LedFrame for MockDriver {
        fn set_color(&mut self, index: usize, rgb: Rgb) {
            self.frame.set_color(index, rgb);
        }
    }

    impl FrameAll for MockDriver {
        fn set_all(&mut self, rgb: Rgb) {
            self.frame.fill(rgb);
        }
    }

    impl LedDriver for MockDriver {
        async fn flush(&mut self) {
            self.flushes += 1;
        }
    }

    fn engine(config: RgbConfig) -> RgbEngine<N, MockDriver> {
        RgbEngine::new(leds(), config, 255, MockDriver::default())
    }

    /// A clock-free `block_on`.
    ///
    /// The engine's `tick` is a plain state machine and `flush` resolves
    /// immediately, so nothing here awaits. `test_support::test_block_on` would
    /// work too, but it drives `embassy-time`'s process-global mock clock and so
    /// only runs under `cargo nextest`; these tests should run under plain
    /// `cargo test` as well.
    fn run<Fut: core::future::Future>(fut: Fut) -> Fut::Output {
        use core::task::{Context, Poll, Waker};

        fn noop_waker() -> Waker {
            use core::task::{RawWaker, RawWakerVTable};
            const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
            const RAW: RawWaker = RawWaker::new(core::ptr::null(), &VTABLE);
            // Safety: every vtable entry is a no-op and no state is read through
            // the null data pointer.
            unsafe { Waker::from_raw(RAW) }
        }

        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut fut = core::pin::pin!(fut);
        for _ in 0..64 {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
        }
        panic!("future did not resolve; the engine should never await");
    }

    /// Run the machine to completion for one frame, returning the flush count.
    async fn run_frame(engine: &mut RgbEngine<N, MockDriver>) -> usize {
        let before = engine.frame.flushes;
        // Starting -> Rendering -> ... -> Flushing -> Syncing.
        for _ in 0..64 {
            let state = engine.tick().await;
            if state == TaskState::Syncing {
                break;
            }
        }
        engine.frame.flushes - before
    }

    #[test]
    fn a_frame_flushes_exactly_once() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            assert_eq!(run_frame(&mut engine).await, 1, "one flush per frame");
        });
    }

    #[test]
    fn every_led_is_written_within_one_frame() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            engine.config_mut().set_hsv(0, 255, 200, 255);
            // SOLID_COLOR: every LED the same colour.
            engine.config_mut().set_mode(2, EFFECT_MAX);

            run_frame(&mut engine).await;

            let expected = crate::rgb::fx::color::hsv_to_rgb(engine.config().hsv());
            assert_eq!(engine.frame.frame.pixels, [expected; N]);
        });
    }

    #[test]
    fn the_flush_limit_holds_the_next_frame_until_enough_time_passed() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            run_frame(&mut engine).await;

            // Immediately after a frame the machine waits in Syncing.
            assert_eq!(engine.tick().await, TaskState::Syncing, "still too soon");

            // Advancing the clock by the limit releases it.
            engine.advance_time(LED_FLUSH_LIMIT_MS);
            assert_eq!(engine.tick().await, TaskState::Starting);
        });
    }

    #[test]
    fn disabling_clears_the_frame_and_stops_the_effect() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            engine.config_mut().set_hsv(0, 255, 255, 255);
            run_frame(&mut engine).await;
            assert_ne!(engine.frame.frame.pixels[0], Rgb::BLACK, "lit while enabled");

            engine.config_mut().disable();
            engine.advance_time(LED_FLUSH_LIMIT_MS);
            run_frame(&mut engine).await;
            assert_eq!(engine.frame.frame.pixels, [Rgb::BLACK; N], "cleared when disabled");
        });
    }

    #[test]
    fn batching_renders_the_whole_chain_across_several_ticks() {
        run(async {
            // 6 LEDs, 2 per batch: three render ticks plus start/flush/sync.
            let mut engine = engine(RgbConfig::default()).with_process_limit(2);
            engine.config_mut().set_hsv(0, 255, 255, 255);
            engine.config_mut().set_mode(2, EFFECT_MAX);

            let mut render_ticks = 0;
            for _ in 0..64 {
                let before = engine.state();
                let state = engine.tick().await;
                if before == TaskState::Rendering {
                    render_ticks += 1;
                }
                if state == TaskState::Syncing {
                    break;
                }
            }

            assert_eq!(render_ticks, 3, "6 LEDs at 2 per batch");
            let expected = crate::rgb::fx::color::hsv_to_rgb(engine.config().hsv());
            assert_eq!(engine.frame.frame.pixels, [expected; N], "every LED was reached");
        });
    }

    #[test]
    fn a_key_event_feeds_the_reactive_effects() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            // SOLID_REACTIVE_SIMPLE: only the hit LED lights.
            engine.config_mut().set_hsv(0, 255, 255, 255);
            engine.config_mut().set_mode(31, EFFECT_MAX);
            // Key (1, 1) is LED index 4 in this table.
            engine.handle_key_event(1, 1, true);

            run_frame(&mut engine).await;

            // A fresh hit is at full brightness, minus `scale8`'s one-part-in-256
            // truncation, so compare against the decayed value rather than
            // `config.hsv` directly.
            let lit = crate::rgb::fx::color::hsv_to_rgb(RgbConfig::default().hsv().with_v(scale8(255, 255)));
            assert_eq!(engine.frame.frame.pixels[4], lit);
            assert_eq!(engine.frame.frame.pixels[0], Rgb::BLACK, "an untouched LED stays dark");
        });
    }

    #[test]
    fn a_key_release_does_not_register_a_hit() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            engine.config_mut().set_hsv(0, 255, 255, 255);
            engine.config_mut().set_mode(31, EFFECT_MAX);
            // `RGB_MATRIX_KEYRELEASES` is off, so only presses count.
            engine.handle_key_event(1, 1, false);

            run_frame(&mut engine).await;
            assert_eq!(engine.frame.frame.pixels, [Rgb::BLACK; N]);
        });
    }

    #[test]
    fn the_engine_starts_in_syncing_and_waits_a_frame_before_drawing() {
        run(async {
            let mut engine = engine(RgbConfig::default());
            assert_eq!(engine.state(), TaskState::Syncing);
        });
    }

    #[test]
    fn config_round_trips_through_the_storage_handles() {
        let mut engine = engine(RgbConfig::default());
        let mut config = RgbConfig::default();
        config.set_mode(24, EFFECT_MAX);
        config.set_hsv(10, 20, 30, 255);
        engine.restore_config(config);

        let stored = engine.config_for_storage();
        assert_eq!(stored.mode(), 24);
        assert_eq!(stored.hue(), 10);
        assert_eq!(stored.val(), 30);
    }

    #[test]
    fn selectability_covers_off_and_the_table() {
        assert!(is_selectable(EFFECT_OFF));
        assert!(is_selectable(2));
        assert!(is_selectable(51), "extension ids are selectable by keycode");
        assert!(!is_selectable(super::super::registry::EFFECT_DIRECT), "not a QMK effect");
        assert!(!is_selectable(EFFECT_MAX));
    }
}
