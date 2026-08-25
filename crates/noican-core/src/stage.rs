//! The processing-stage abstraction.
//!
//! Every enhancement model in noican — noise suppression, speaker suppression,
//! a future echo canceller, or a plain bypass — is a [`Stage`]. Adding a model
//! therefore means writing one trait implementation and registering it; nothing
//! in the engine, the CLI, or the UI needs to change.

use crate::error::Result;

/// How a stage may be used.
///
/// Some published models only ship ONNX graphs that process a whole sequence
/// with zero-initialised recurrent state. Those are perfectly usable for
/// offline comparison but cannot be dropped into a 10 ms audio path, and the
/// engine must be able to tell the difference before it hands a stage to the
/// real-time thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageCapability {
    /// Frame-streaming: recurrent state is threaded across calls, so the stage
    /// produces the same output whether it is fed one block or a whole file.
    /// Safe for the live microphone path.
    Streaming,

    /// Block processing: the stage needs a large block and resets its internal
    /// state between calls. Usable in the CLI and (with the block's worth of
    /// latency) in the live path, but not recommended for it.
    Block,
}

impl StageCapability {
    /// Whether this stage is appropriate for the live microphone path.
    #[must_use]
    pub const fn is_realtime(self) -> bool {
        matches!(self, Self::Streaming)
    }
}

/// The fixed properties of a stage, queried once when it is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageSpec {
    /// Rate at which [`Stage::process`] expects and produces samples, in hertz.
    pub sample_rate: u32,

    /// Number of samples consumed from the input and written to the output on
    /// every [`Stage::process`] call.
    pub block_size: usize,

    /// Algorithmic delay the stage introduces, in samples at `sample_rate`.
    ///
    /// This is the group delay of the model itself (analysis lookahead, filter
    /// order, and so on), not the block size. Offline comparisons trim it so
    /// that outputs line up with the input; the live path simply pays it.
    pub latency_samples: usize,

    /// Whether the stage may run on the audio path.
    pub capability: StageCapability,
}

impl StageSpec {
    /// Creates a streaming spec with no extra algorithmic delay.
    #[must_use]
    pub const fn streaming(sample_rate: u32, block_size: usize) -> Self {
        Self {
            sample_rate,
            block_size,
            latency_samples: 0,
            capability: StageCapability::Streaming,
        }
    }

    /// Returns a copy with `latency_samples` replaced.
    #[must_use]
    pub const fn with_latency(mut self, latency_samples: usize) -> Self {
        self.latency_samples = latency_samples;
        self
    }

    /// Returns a copy with `capability` replaced.
    #[must_use]
    pub const fn with_capability(mut self, capability: StageCapability) -> Self {
        self.capability = capability;
        self
    }

    /// The stage's block size expressed in milliseconds.
    #[must_use]
    pub fn block_duration_ms(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "block sizes and sample rates are far below f32's exact-integer limit"
        )]
        let ms = (self.block_size as f32 / self.sample_rate as f32) * 1000.0;
        ms
    }

    /// The stage's algorithmic delay expressed in milliseconds.
    #[must_use]
    pub fn latency_ms(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "latencies and sample rates are far below f32's exact-integer limit"
        )]
        let ms = (self.latency_samples as f32 / self.sample_rate as f32) * 1000.0;
        ms
    }
}

/// A single mono audio-processing step.
///
/// # Contract
///
/// * [`Stage::process`] is called with `input.len() == output.len() ==
///   spec().block_size`. Implementations may assume nothing else about the
///   buffers, and must not retain references to them.
/// * Samples are mono `f32` in `[-1.0, 1.0]`, at `spec().sample_rate`.
/// * `process` must be real-time safe when [`StageSpec::capability`] is
///   [`StageCapability::Streaming`]: no allocation, no locks, no I/O. All
///   scratch space is allocated when the stage is constructed.
/// * [`Stage::reset`] returns the stage to its initial state and may allocate.
pub trait Stage: Send {
    /// Fixed properties of this stage.
    fn spec(&self) -> StageSpec;

    /// Processes exactly one block.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer lengths do not match
    /// [`StageSpec::block_size`], or if the underlying model fails.
    fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<()>;

    /// Clears all internal state, as if the stage had just been constructed.
    fn reset(&mut self);
}

/// A stage that copies its input unchanged.
///
/// Used as the "off" position of the model selector and as the reference row in
/// offline comparisons.
#[derive(Debug, Clone, Copy)]
pub struct Passthrough {
    spec: StageSpec,
}

impl Passthrough {
    /// Creates a passthrough stage for the given rate and block size.
    #[must_use]
    pub const fn new(sample_rate: u32, block_size: usize) -> Self {
        Self {
            spec: StageSpec::streaming(sample_rate, block_size),
        }
    }
}

impl Stage for Passthrough {
    fn spec(&self) -> StageSpec {
        self.spec
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<()> {
        if input.len() != output.len() {
            return Err(crate::Error::BufferLength {
                expected: input.len(),
                actual: output.len(),
            });
        }
        output.copy_from_slice(input);
        Ok(())
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "the passthrough stage only copies samples, so bit equality is exactly the property \
              these assertions check"
)]
mod tests {
    use super::{Passthrough, Stage, StageCapability, StageSpec};

    #[test]
    fn passthrough_copies_input() {
        let mut stage = Passthrough::new(48_000, 4);
        let input = [0.1, -0.2, 0.3, -0.4];
        let mut output = [0.0; 4];
        stage.process(&input, &mut output).unwrap();
        assert_eq!(input, output);
    }

    #[test]
    fn passthrough_rejects_mismatched_buffers() {
        let mut stage = Passthrough::new(48_000, 4);
        let mut output = [0.0; 3];
        assert!(stage.process(&[0.0; 4], &mut output).is_err());
    }

    #[test]
    fn spec_reports_durations() {
        let spec = StageSpec::streaming(48_000, 480).with_latency(1_920);
        assert!((spec.block_duration_ms() - 10.0).abs() < 1e-4);
        assert!((spec.latency_ms() - 40.0).abs() < 1e-4);
        assert!(spec.capability.is_realtime());
        assert!(!StageCapability::Block.is_realtime());
    }
}
