//! The common stage interface every model implements.

use crate::error::StageError;

/// See docs/adr/0012-app-state-reducer.md.
pub const ENGINE_SAMPLE_RATE: u32 = 48_000;

/// See docs/adr/0007-dsp-stages.md.
pub trait Stage: Send {
    /// Stable identifier of the underlying model/configuration.
    fn id(&self) -> &str;

    /// See docs/adr/0007-dsp-stages.md.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::BufferLen`] when `output.len() != input.len()`,
    /// or [`StageError::Inference`] when the backend fails.
    fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError>;

    /// See docs/adr/0002-engine-diagnostics.md.
    fn latency_samples(&self) -> usize;

    /// Drop all internal state (buffers, model recurrent state).
    fn reset(&mut self);
}

/// See docs/adr/0007-dsp-stages.md.
pub trait FrameProcessor: Send {
    /// Stable identifier of the underlying model/configuration.
    fn id(&self) -> &str;

    /// See docs/adr/0012-app-state-reducer.md.
    fn sample_rate(&self) -> u32;

    /// Samples consumed and produced per `process_frame` call, at
    /// [`Self::sample_rate`].
    fn frame_len(&self) -> usize;

    /// See docs/adr/0007-dsp-stages.md.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the backend fails.
    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError>;

    /// See docs/adr/0002-engine-diagnostics.md.
    fn output_delay(&self) -> usize {
        0
    }

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
