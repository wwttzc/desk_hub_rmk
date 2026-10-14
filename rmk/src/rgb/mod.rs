//! RGB Matrix lighting: a Rust port of QMK's `rgb_matrix` subsystem.
//!
//! This module is chip-independent. It knows about LED colours, effect state
//! and the Vial-side protocol, but never about a GPIO, an RMT channel or a
//! timer — the board-specific driver is injected from the user's crate, and
//! time comes in as a parameter. That keeps the effect code identical across
//! chips and lets the whole engine run under `cargo test` on the host.
//!
//! Layout mirrors QMK's own split:
//!
//! - [`fx`] — the `lib8tion` integer math and colour conversion an effect is
//!   written in terms of.
//! - (planned) `led_config`, `state`, `runner`, `effects`, `vialrgb`.

pub mod fx;
