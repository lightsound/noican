//! Core audio-processing abstractions for the Noican engine.
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
//! - [`resample`]: streaming polyphase FIR decimator/interpolator
//!   (integer factors, model path) and the arbitrary-ratio polyphase
//!   resampler (capture path).
//! - [`capture`]: native-rate microphone capture support — the input
//!   resampler (any native rate to 48 kHz, drift correction folded in)
//!   and the ring-occupancy drift servo for non-48 kHz microphones
//!   (issue #7).
//! - [`switch::SwitchingEngine`]: lock-free, click-free runtime switching
//!   between prepared stages (the live-pipeline building block).
//! - [`mix::IntensityControl`]: the atomic dry/wet ("strength") control
//!   whose blend runs inside the switching engine, with the dry path
//!   delay-compensated by the active stage's reported latency.

pub mod capture;
pub mod error;
pub mod framed;
pub mod mix;
pub mod resample;
pub mod stage;
pub mod switch;

pub use capture::{DriftServo, InputResampler};
pub use error::StageError;
pub use framed::FramedStage;
pub use mix::IntensityControl;
pub use stage::{ENGINE_SAMPLE_RATE, FrameProcessor, Passthrough, Stage};
pub use switch::{StagePublisher, SwitchingEngine};
