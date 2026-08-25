//! Common contracts implemented by every audio model.

use thiserror::Error;

/// Sample rate used at the boundary of every noican pipeline.
pub const PIPELINE_SAMPLE_RATE: u32 = 48_000;

/// Processing quantum shared by the live pipeline and offline comparison path.
pub const PIPELINE_FRAME_SAMPLES: usize = 480;

/// Broad purpose of a model stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageKind {
    /// Removes non-speech background noise.
    NoiseSuppression,
    /// Isolates the foreground or enrolled speaker.
    SpeakerSuppression,
}

/// Enrollment data required before a stage can run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnrollmentRequirement {
    /// The model can run without user-specific data.
    None,
    /// The model consumes a fixed-width speaker embedding.
    SpeakerEmbedding {
        /// Number of scalar values in the embedding.
        dimensions: usize,
    },
}

/// Static and runtime-relevant facts about an audio stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageDescriptor {
    /// Stable machine-readable model identifier.
    pub id: &'static str,
    /// Human-readable model name shown by the UI.
    pub display_name: &'static str,
    /// Model purpose.
    pub kind: StageKind,
    /// Native model sample rate.
    pub sample_rate: u32,
    /// Exact number of samples accepted and emitted by one model call.
    pub frame_samples: usize,
    /// Delay introduced by the model, measured at `sample_rate`.
    pub algorithmic_delay_samples: usize,
    /// Additional zero frames needed to drain recurrent or overlap-add state.
    pub tail_frames: usize,
    /// Enrollment data needed by the model.
    pub enrollment: EnrollmentRequirement,
}

/// Failures produced while preparing or running a stage.
#[derive(Debug, Error)]
pub enum StageError {
    /// A stage received a frame with the wrong number of samples.
    #[error("stage {stage} expected {expected} samples, received {actual}")]
    InvalidFrameLength {
        /// Stable stage identifier.
        stage: &'static str,
        /// Required frame size.
        expected: usize,
        /// Supplied frame size.
        actual: usize,
    },
    /// A model descriptor contains values that cannot form a pipeline.
    #[error("invalid stage configuration for {stage}: {message}")]
    InvalidConfiguration {
        /// Stable stage identifier.
        stage: &'static str,
        /// Configuration failure detail.
        message: String,
    },
    /// A model backend failed.
    #[error("{stage} backend failed: {message}")]
    Backend {
        /// Stable stage identifier.
        stage: &'static str,
        /// Backend failure detail.
        message: String,
    },
    /// Sample-rate conversion failed.
    #[error("sample-rate conversion failed: {0}")]
    Resampling(String),
}

/// A stateful, fixed-frame audio model.
///
/// Implementations operate at their native rate. [`crate::RateAdapter`] absorbs
/// rate and frame-size differences and exposes the shared 48 kHz pipeline
/// contract.
pub trait AudioStage: Send {
    /// Describe the model and its exact frame contract.
    fn descriptor(&self) -> StageDescriptor;

    /// Process exactly one native frame.
    ///
    /// `input` and `output` must both contain `descriptor().frame_samples`
    /// values. Implementations retain recurrent state between calls.
    ///
    /// # Errors
    ///
    /// Returns [`StageError`] when frame validation or inference fails.
    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError>;

    /// Reset recurrent and overlap state between independent streams.
    ///
    /// # Errors
    ///
    /// Returns [`StageError`] if the backend cannot reset cleanly.
    fn reset(&mut self) -> Result<(), StageError>;
}

/// Validate the fixed-frame slices supplied to a stage.
///
/// # Errors
///
/// Returns [`StageError::InvalidFrameLength`] for either mismatched slice.
pub fn validate_frame_lengths(
    descriptor: StageDescriptor,
    input: &[f32],
    output: &[f32],
) -> Result<(), StageError> {
    for actual in [input.len(), output.len()] {
        if actual != descriptor.frame_samples {
            return Err(StageError::InvalidFrameLength {
                stage: descriptor.id,
                expected: descriptor.frame_samples,
                actual,
            });
        }
    }
    Ok(())
}
