//! Stage for models whose ONNX graph takes and returns a waveform.
//!
//! `FastEnhancer` is the only such model in the catalog: its export embeds a
//! `DFT` node, so no transform is needed on our side. All five published
//! variants share this implementation because the cache shapes — which differ
//! between them — are read from the graph rather than hard-coded.

use noican_core::{Result as CoreResult, Stage, StageSpec};

use crate::error::{Error, Result};
use crate::session::CachedSession;

/// A waveform-in, waveform-out streaming model.
#[derive(Debug)]
pub struct WaveformStage {
    session: CachedSession,
    spec: StageSpec,
    /// The shape the graph declares for its waveform input, typically
    /// `[1, block_size]`.
    io_shape: Vec<i64>,
}

impl WaveformStage {
    /// Loads the graph at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedSignature`] if the graph does not take one
    /// waveform input of static length followed by state tensors, or
    /// [`Error::Runtime`] if it cannot be loaded.
    pub fn load(
        model_id: &str,
        path: &std::path::Path,
        sample_rate: u32,
        latency_samples: usize,
    ) -> Result<Self> {
        let session = CachedSession::load(model_id, path, 1, 1)?;
        let io_shape = session.primary_input_shape(0).to_vec();

        let block_size = io_shape
            .last()
            .copied()
            .and_then(|length| usize::try_from(length).ok())
            .filter(|&length| length > 0)
            .ok_or_else(|| Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "waveform input has shape {io_shape:?}; the block size must be static"
                ),
            })?;

        Ok(Self {
            session,
            spec: StageSpec::streaming(sample_rate, block_size).with_latency(latency_samples),
            io_shape,
        })
    }
}

impl Stage for WaveformStage {
    fn spec(&self) -> StageSpec {
        self.spec
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> CoreResult<()> {
        if input.len() != self.spec.block_size || output.len() != self.spec.block_size {
            return Err(noican_core::Error::BufferLength {
                expected: self.spec.block_size,
                actual: input.len().min(output.len()),
            });
        }

        let mut destinations = [output];
        self.session
            .run(&[(self.io_shape.as_slice(), input)], &mut destinations)
            .map_err(|error| noican_core::Error::Stage(error.to_string()))
    }

    fn reset(&mut self) {
        self.session.reset();
    }
}
