//! The integer math an RGB Matrix effect is built from, ported from QMK's
//! `lib/lib8tion` and `quantum/color`.
//!
//! These modules are deliberately dependency-free and `no_std`: an effect only
//! ever needs bytes, so nothing here touches a HAL, a timer, or the LED chain.
//! That also makes the whole layer testable on the host, which is how the QMK
//! edge-case behaviour is pinned down in unit tests.
//!
//! Ports always take QMK's portable C branch rather than its AVR/ARM assembly;
//! see each module's documentation for the behaviour switches that matter.

pub mod color;
pub mod math8;
pub mod random8;
pub mod trig8;
