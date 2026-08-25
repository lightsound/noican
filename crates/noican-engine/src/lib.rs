//! The real-time engine: model switching without clicks, and hand-off between
//! the audio callback and the inference thread.
//!
//! Three threads touch this crate, and the split between them is the whole
//! design:
//!
//! * The **audio callback**, driven by Core Audio, only moves samples through
//!   lock-free queues ([`AudioBridge`]). It never allocates, locks, or runs
//!   inference — `docs/tech-research.md` §9 forbids all three.
//! * The **inference thread** owns the active [`noican_core::StageRunner`], runs
//!   the model, and performs the switch ramp. It may not block.
//! * The **control thread** (the UI) builds and destroys runners, which is where
//!   all allocation happens. A retired runner is handed back to it for disposal
//!   rather than dropped on the inference thread.
//!
//! This crate is deliberately platform-independent, so the switching logic is
//! testable without a Mac.

pub mod bridge;
pub mod engine;
pub mod error;
pub mod status;
pub mod switch;

pub use bridge::AudioBridge;
pub use engine::{Engine, EngineConfig};
pub use error::{Error, Result};
pub use status::{Snapshot, Status};
pub use switch::SwitchRamp;
