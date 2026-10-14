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
//! - [`led_config`] — the LED table: positions, flags and key mapping.
//! - [`state`] — the run-time configuration and effect-switching rules.
//! - [`hit_tracker`] — recent key hits, the input to reactive effects.
//! - [`runner`] — the generic runners and the batched-iteration contract.
//! - [`effects`] — the effect bodies themselves, ported one per QMK file.
//! - [`registry`] — the effect table: ids, names, config keys and dispatch.
//! - [`engine`] — the task that renders frames and drives a LED driver.
//! - [`vialrgb`] — the VialRGB wire protocol: framing, decoding and encoding.
//! - [`shared`] — the state the protocol and the engine both reach.
//! - [`via`] — the VIA command handlers that connect the two.
//! - [`storage`] — persistence for the VIA `CustomSave` (0x09) path.
//! - [`drivers`] — the bridge to a `smart-leds` driver such as
//!   `esp-hal-smartled`.

pub mod effects;
pub mod engine;
pub mod fx;
pub mod hit_tracker;
pub mod led_config;
pub mod registry;
pub mod runner;
pub mod shared;
pub mod state;
#[cfg(feature = "storage")]
pub mod storage;
pub mod via;
pub mod vialrgb;

#[cfg(feature = "smart_leds")]
pub mod drivers;
