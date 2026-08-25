//! FastEnhancer 48 kHz streaming stage (wav2wav ONNX).
//!
//! Protocol (verified against the official `scripts/test_onnx.py` and a
//! Python smoke test; see docs/tech-research.md §5.2 and docs/models.md):
//! feed `hop` raw waveform samples per call (`wav_in [1, hop]`), receive
//! `hop` enhanced samples (`wav_out`), and thread every `cache_out_j` back
//! into `cache_in_j`. STFT/iSTFT and feature handling live inside the
//! graph. Output lags input by `n_fft - hop` samples, which equals the
//! size of `cache_in_0` (the STFT input ring buffer).

use std::borrow::Cow;
use std::path::Path;

use noican_core::{FrameProcessor, StageError};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::onnx::{StateBank, inference_error, input_shape, load_streaming_session};

/// FastEnhancer streaming frame processor.
#[derive(Debug)]
pub struct FastEnhancerStage {
    id: String,
    session: Session,
    hop: usize,
    /// `n_fft - hop`, learned from the `cache_in_0` shape.
    output_delay: usize,
    states: StateBank,
}

impl FastEnhancerStage {
    /// Loads the wav2wav streaming ONNX at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the file cannot be loaded or
    /// does not expose the expected tensors.
    pub fn new(id: &str, path: &Path) -> Result<Self, StageError> {
        let session = load_streaming_session(path)?;
        let wav_in = input_shape(&session, "wav_in")?;
        let hop = *wav_in
            .get(1)
            .ok_or_else(|| StageError::Inference("wav_in is not [1, hop]".to_owned()))?;
        let states = StateBank::from_indexed_prefix(&session, "cache_in_", "cache_out_")?;
        if states.is_empty() {
            return Err(StageError::Inference(
                "no cache_in_* tensors found; not a FastEnhancer streaming export".to_owned(),
            ));
        }
        let output_delay = input_shape(&session, "cache_in_0")?.iter().product();
        Ok(Self {
            id: id.to_owned(),
            session,
            hop,
            output_delay,
            states,
        })
    }
}

impl FrameProcessor for FastEnhancerStage {
    fn id(&self) -> &str {
        &self.id
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frame_len(&self) -> usize {
        self.hop
    }

    fn output_delay(&self) -> usize {
        self.output_delay
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        let mut inputs: Vec<(Cow<'static, str>, SessionInputValue<'static>)> =
            Vec::with_capacity(1 + self.states.len());
        let wav = Tensor::from_array(([1, self.hop], input.to_vec()))
            .map_err(|e| inference_error("building wav_in", &e))?;
        inputs.push((Cow::Borrowed("wav_in"), wav.into()));
        self.states.append_inputs(&mut inputs)?;
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| inference_error("FastEnhancer inference", &e))?;
        let (_, data) = outputs
            .get("wav_out")
            .ok_or_else(|| StageError::Inference("wav_out missing".to_owned()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| inference_error("extracting wav_out", &e))?;
        if data.len() != output.len() {
            return Err(StageError::BufferLen {
                expected: output.len(),
                got: data.len(),
            });
        }
        output.copy_from_slice(data);
        self.states.update_from_outputs(&outputs)?;
        Ok(())
    }

    fn reset(&mut self) {
        self.states.reset();
    }
}
