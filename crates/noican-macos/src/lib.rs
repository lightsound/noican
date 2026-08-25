//! Core Audio capture, routing, and aggregate-device management.
//!
//! Everything here is macOS-only. Away from macOS the crate still compiles —
//! so that `cargo clippy --workspace` and `cargo test --workspace` mean
//! something on Linux CI — but every entry point returns [`Error::Unsupported`]
//! and the pure logic (device filtering, channel de-interleaving, the callback's
//! buffer handling) is tested there.
//!
//! # The shape of the audio path
//!
//! ```text
//! physical mic ─┐
//!               ├─ private aggregate device ── one I/O proc ── `AudioBridge`
//! virtual out ──┘   (drift compensation)                            │
//!                                                        inference thread
//! ```
//!
//! Both devices live inside one aggregate so that a single callback services
//! them and they share a clock domain; see [`aggregate`] for why that is
//! mandatory rather than convenient.

// Core Audio is a C API. Nothing in this crate can reach it without `unsafe`,
// and there is no safe wrapper worth depending on for the dozen calls we need
// (`sys`). Every `unsafe` block below carries a `SAFETY:` note stating what the
// caller or Core Audio guarantees; the rest of the workspace keeps
// `unsafe_code` denied, and this is the only crate besides the C ABI shim that
// relaxes it.
#![expect(
    unsafe_code,
    reason = "the Core Audio HAL is a C API with no safe binding worth depending on"
)]

pub mod aggregate;
pub mod devices;
pub mod error;
pub mod io;
pub mod session;
pub mod sys;

pub use aggregate::AggregateDevice;
pub use devices::Device;
pub use error::{Error, Result};
pub use io::{IoStream, StreamConfig};
pub use session::{Session, SessionConfig};
