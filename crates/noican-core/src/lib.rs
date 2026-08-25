//! Core audio-processing abstractions for the noican engine.
//!
//! This crate is platform-independent and free of inference-runtime
//! dependencies. It defines the common stage interface that every
//! noise-suppression / speaker-suppression model implements, plus the
//! sample-rate and frame-size adaptation that lets the engine treat all
//! models identically (docs/tech-research.md §12, Phase 0).
//!
//! - [`stage::Stage`]: streaming processor at the fixed engine rate
//!   ([`stage::ENGINE_SAMPLE_RATE`], 48 kHz), arbitrary block sizes.
//! - [`stage::FrameProcessor`]: what a model actually implements — fixed
//!   frame size at its native rate.
//! - [`framed::FramedStage`]: adapts a `FrameProcessor` to a `Stage`,
//!   handling resampling (integer factors) and frame accumulation.
//! - [`resample`]: streaming polyphase FIR decimator/interpolator.

pub mod error;
pub mod framed;
pub mod resample;
pub mod stage;

pub use error::StageError;
pub use framed::FramedStage;
pub use stage::{ENGINE_SAMPLE_RATE, FrameProcessor, Passthrough, Stage};
