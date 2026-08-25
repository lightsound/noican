//! The common stage interface every model implements.

use crate::error::StageError;

/// The fixed sample rate (Hz) at which the engine exchanges audio with
/// stages. Models running at other rates are wrapped by
/// [`crate::framed::FramedStage`], which resamples internally.
pub const ENGINE_SAMPLE_RATE: u32 = 48_000;

/// A streaming mono audio processor operating at [`ENGINE_SAMPLE_RATE`].
///
/// Implementations must be suitable for use on a dedicated inference thread:
/// `process_block` may allocate internally (it does not run on the audio I/O
/// callback; see docs/tech-research.md §9), but should avoid unbounded work.
pub trait Stage: Send {
    /// Stable identifier of the underlying model/configuration.
    fn id(&self) -> &str;

    /// Process one block. `output` must be the same length as `input` and is
    /// fully overwritten. The stage may impose internal latency: the first
    /// samples it emits are zeros until its pipeline fills.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::BufferLen`] when `output.len() != input.len()`,
    /// or [`StageError::Inference`] when the backend fails.
    fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError>;

    /// Total internal latency in samples at [`ENGINE_SAMPLE_RATE`]
    /// (resampling + frame buffering; excludes the model's own algorithmic
    /// lookahead unless the implementation accounts for it).
    fn latency_samples(&self) -> usize;

    /// Drop all internal state (buffers, model recurrent state).
    fn reset(&mut self);
}

/// A fixed-frame mono processor at its native sample rate.
///
/// Model implementations implement this; [`crate::framed::FramedStage`]
/// adapts it to the [`Stage`] interface, absorbing rate and frame-size
/// differences.
pub trait FrameProcessor: Send {
    /// Stable identifier of the underlying model/configuration.
    fn id(&self) -> &str;

    /// Native sample rate (Hz). Must divide [`ENGINE_SAMPLE_RATE`].
    fn sample_rate(&self) -> u32;

    /// Samples consumed and produced per `process_frame` call, at
    /// [`Self::sample_rate`].
    fn frame_len(&self) -> usize;

    /// Process exactly one frame: `input.len() == output.len() ==`
    /// [`Self::frame_len`].
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the backend fails.
    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError>;

    /// Drop recurrent model state.
    fn reset(&mut self);
}

/// Identity stage: forwards input unchanged. Used as the "off"/bypass model
/// and as a latency-free reference in comparisons.
#[derive(Debug, Default, Clone, Copy)]
pub struct Passthrough;

impl Stage for Passthrough {
    fn id(&self) -> &'static str {
        "passthrough"
    }

    fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        if input.len() != output.len() {
            return Err(StageError::BufferLen {
                expected: input.len(),
                got: output.len(),
            });
        }
        output.copy_from_slice(input);
        Ok(())
    }

    fn latency_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {}
}
