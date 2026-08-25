//! Model-switchable audio processing for noican.
//!
//! Model implementations operate on fixed native frames behind
//! [`AudioStage`]. [`RateAdapter`] normalizes every implementation to a 48 kHz,
//! 10 ms contract, while [`SwitchingEngine`] activates prepared replacements
//! through a bounded lock-free queue and a short mute transition.

mod offline;
mod rate;
mod stage;
mod r#switch;

pub use offline::{process_clip, DelayCompensation};
pub use r#switch::{StagePublisher, SwitchingEngine};
pub use rate::RateAdapter;
pub use stage::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind, PIPELINE_FRAME_SAMPLES, PIPELINE_SAMPLE_RATE,
};
