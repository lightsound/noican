//! See docs/adr/0007-dsp-stages.md#core-engine-architecture.

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
