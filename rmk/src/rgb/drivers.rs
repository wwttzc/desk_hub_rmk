//! The bridge between the engine's frame buffer and a `smart-leds` driver.
//!
//! [`RmtSmartLeds`](https://docs.rs/esp-hal-smartled) on ESP32, and the
//! equivalents on other chips, implement `smart-leds-trait`'s write traits
//! rather than this crate's [`LedDriver`]. This adapter is the one place that
//! translates between them, so the engine never has to know which driver it has
//! and a board only has to construct one.
//!
//! It is deliberately chip-agnostic: the generated board code constructs the
//! concrete driver and hands it here, exactly as it does for the USB driver.
//!
//! # Blocking versus async
//!
//! `esp-hal-smartled` implements whichever of the two traits matches its RMT
//! channel mode. This adapter takes the blocking one, because:
//!
//! - the async path needs `ChannelCreator::into_async()` plus an RMT interrupt
//!   binding, which is board setup the engine should not dictate;
//! - a 19-LED frame is about 570 us on the wire, so at the 60 fps ceiling
//!   [`LED_FLUSH_LIMIT_MS`](super::engine::LED_FLUSH_LIMIT_MS) imposes, the
//!   blocking cost is roughly 3% of wall time.
//!
//! QMK blocks in the same place. If a board measures a problem — BLE timing is
//! the likely one — the switch is to make `RmtSmartLeds` async and add a
//! `SmartLedsWriteAsync` implementation of `LedDriver` here. Both traits cannot
//! be implemented on one type, because a driver that implements both would make
//! the two impls overlap; the async variant would therefore be a second adapter
//! type rather than a second impl on this one.

use smart_leds_trait::{RGB8, SmartLedsWrite};

use super::engine::LedDriver;
use super::fx::color::Rgb;
use super::runner::{FrameAll, LedFrame};

/// A frame buffer backed by a `smart-leds` driver.
///
/// Effects write into `pixels`; `flush` hands the whole array to `driver` in
/// one write, which is what the RMT peripheral needs to emit a single frame with
/// one reset pulse at the end.
pub struct SmartLedsFrame<const LED_COUNT: usize, D> {
    pixels: [Rgb; LED_COUNT],
    driver: D,
}

impl<const LED_COUNT: usize, D> SmartLedsFrame<LED_COUNT, D> {
    /// Wrap a driver. The pixels start black, which is what a chain shows after
    /// power-on until the first frame.
    pub const fn new(driver: D) -> Self {
        Self {
            pixels: [Rgb::BLACK; LED_COUNT],
            driver,
        }
    }

    /// The driver, for a caller that wants it back (for example to `flush` by
    /// hand in a test).
    pub fn into_driver(self) -> D {
        self.driver
    }

    /// The current pixels.
    pub fn pixels(&self) -> &[Rgb; LED_COUNT] {
        &self.pixels
    }
}

impl<const LED_COUNT: usize, D> LedFrame for SmartLedsFrame<LED_COUNT, D> {
    #[inline]
    fn set_color(&mut self, index: usize, rgb: Rgb) {
        self.pixels[index] = rgb;
    }
}

impl<const LED_COUNT: usize, D> FrameAll for SmartLedsFrame<LED_COUNT, D> {
    fn set_all(&mut self, rgb: Rgb) {
        self.pixels = [rgb; LED_COUNT];
    }
}

impl<const LED_COUNT: usize, D> LedDriver for SmartLedsFrame<LED_COUNT, D>
where
    D: SmartLedsWrite<Color = RGB8>,
{
    async fn flush(&mut self) {
        // The driver's byte order is a compile-time parameter of the driver, so
        // this conversion is a plain field copy: the RGB vs GRB decision happens
        // inside `write`.
        let frame = self.pixels.iter().map(|p| RGB8 {
            r: p.r,
            g: p.g,
            b: p.b,
        });
        if self.driver.write(frame).is_err() {
            // A failed frame leaves the previous one on the wire, which is the
            // least surprising outcome; the next frame retries. Logged at debug
            // because it repeats every frame while the fault lasts.
            debug!("WS2812 write failed; the LEDs keep the previous frame");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A driver that records the frames it was given.
    #[derive(Default)]
    struct RecordingDriver {
        frames: Vec<Vec<RGB8>>,
        fail: bool,
    }

    impl SmartLedsWrite for RecordingDriver {
        type Error = ();
        type Color = RGB8;

        fn write<T, I>(&mut self, iterator: T) -> Result<(), Self::Error>
        where
            T: IntoIterator<Item = I>,
            I: Into<Self::Color>,
        {
            let frame: Vec<RGB8> = iterator.into_iter().map(Into::into).collect();
            self.frames.push(frame);
            if self.fail { Err(()) } else { Ok(()) }
        }
    }

    fn drive(fut: impl core::future::Future<Output = ()>) {
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
        assert!(
            matches!(fut.as_mut().poll(&mut cx), Poll::Ready(())),
            "the blocking driver never suspends"
        );
    }

    #[test]
    fn a_fresh_frame_is_black() {
        let frame = SmartLedsFrame::<4, _>::new(RecordingDriver::default());
        assert_eq!(frame.pixels(), &[Rgb::BLACK; 4]);
    }

    #[test]
    fn flushing_hands_the_whole_frame_to_the_driver() {
        let mut frame = SmartLedsFrame::<3, _>::new(RecordingDriver::default());
        frame.set_color(0, Rgb::new(1, 2, 3));
        frame.set_color(2, Rgb::new(4, 5, 6));

        drive(frame.flush());

        let driver = frame.into_driver();
        assert_eq!(driver.frames.len(), 1, "one write per flush");
        assert_eq!(
            driver.frames[0],
            vec![
                RGB8 { r: 1, g: 2, b: 3 },
                RGB8 { r: 0, g: 0, b: 0 },
                RGB8 { r: 4, g: 5, b: 6 },
            ]
        );
    }

    /// `rgb_matrix_none` fills every LED at once; the adapter must forward that
    /// as a full frame rather than one write per LED.
    #[test]
    fn set_all_reaches_every_led() {
        let mut frame = SmartLedsFrame::<5, _>::new(RecordingDriver::default());
        frame.set_all(Rgb::new(9, 9, 9));
        assert_eq!(frame.pixels(), &[Rgb::new(9, 9, 9); 5]);

        drive(frame.flush());
        let driver = frame.into_driver();
        assert_eq!(driver.frames[0].len(), 5);
        assert!(driver.frames[0].iter().all(|c| c.r == 9 && c.g == 9 && c.b == 9));
    }

    #[test]
    fn consecutive_flushes_send_consecutive_frames() {
        let mut frame = SmartLedsFrame::<2, _>::new(RecordingDriver::default());

        frame.set_all(Rgb::new(1, 0, 0));
        drive(frame.flush());
        frame.set_all(Rgb::new(0, 1, 0));
        drive(frame.flush());

        let driver = frame.into_driver();
        assert_eq!(driver.frames.len(), 2);
        assert_eq!(driver.frames[0][0], RGB8 { r: 1, g: 0, b: 0 });
        assert_eq!(driver.frames[1][0], RGB8 { r: 0, g: 1, b: 0 });
    }

    /// A driver error must not panic: the LEDs simply keep the previous frame.
    #[test]
    fn a_driver_error_is_swallowed() {
        let mut frame = SmartLedsFrame::<2, _>::new(RecordingDriver {
            fail: true,
            ..Default::default()
        });
        frame.set_all(Rgb::new(7, 7, 7));
        drive(frame.flush());
        assert_eq!(frame.into_driver().frames.len(), 1, "the write was attempted");
    }
}
