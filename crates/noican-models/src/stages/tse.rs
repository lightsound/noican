//! tse-conv-tasnet-48k target-speaker-extraction stage.
//!
//! Causal streaming Conv-TasNet conditioned on a frozen 192-dim ECAPA-TDNN
//! enrollment embedding (`FiLM`). The ONNX export is purpose-built for ONNX
//! Runtime streaming: fixed 480-sample (10 ms @ 48 kHz) chunks with 89
//! explicit state tensors threaded output → input, all zero-initialized.
//!
//! Protocol verified against a structurally identical random-weight export
//! produced by mellonella's `scripts/export_tse_onnx.py` (PyTorch↔ORT
//! parity max|Δ| = 3.4e-8). NOTE: the trained weights on Hugging Face
//! (`penta2himajin/tse-conv-tasnet-48k`) currently return HTTP 401
//! (private repo); see docs/models.md for how to supply the files.

use std::borrow::Cow;
use std::path::Path;

use noican_core::{FrameProcessor, StageError};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::onnx::{StateBank, inference_error, input_shape, load_streaming_session};

/// Dimensionality of the enrollment embedding.
pub const EMBEDDING_DIM: usize = 192;

/// TSE streaming frame processor.
#[derive(Debug)]
pub struct TseStage {
    id: String,
    session: Session,
    chunk: usize,
    embedding: Vec<f32>,
    states: StateBank,
}

impl TseStage {
    /// Loads the TSE streaming ONNX at `path` (its `.onnx.data` sidecar
    /// must sit next to it) with the given enrollment `embedding`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on load failure and
    /// [`StageError::BufferLen`] when the embedding is not 192-dim.
    pub fn new(id: &str, path: &Path, embedding: &[f32]) -> Result<Self, StageError> {
        if embedding.len() != EMBEDDING_DIM {
            return Err(StageError::BufferLen {
                expected: EMBEDDING_DIM,
                got: embedding.len(),
            });
        }
        let session = load_streaming_session(path)?;
        let chunk_shape = input_shape(&session, "audio_chunk")?;
        let chunk = *chunk_shape
            .get(1)
            .ok_or_else(|| StageError::Inference("audio_chunk is not [1, chunk]".to_owned()))?;
        let states = StateBank::from_indexed_prefix(&session, "state_in_", "state_out_")?;
        if states.is_empty() {
            return Err(StageError::Inference(
                "no state_in_* tensors found; not a TSE streaming export".to_owned(),
            ));
        }
        Ok(Self {
            id: id.to_owned(),
            session,
            chunk,
            embedding: embedding.to_vec(),
            states,
        })
    }
}

impl FrameProcessor for TseStage {
    fn id(&self) -> &str {
        &self.id
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frame_len(&self) -> usize {
        self.chunk
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        let mut inputs: Vec<(Cow<'static, str>, SessionInputValue<'static>)> =
            Vec::with_capacity(2 + self.states.len());
        let audio = Tensor::from_array(([1, self.chunk], input.to_vec()))
            .map_err(|e| inference_error("building audio_chunk", &e))?;
        let cond = Tensor::from_array(([1, EMBEDDING_DIM], self.embedding.clone()))
            .map_err(|e| inference_error("building cond_embedding", &e))?;
        inputs.push((Cow::Borrowed("audio_chunk"), audio.into()));
        inputs.push((Cow::Borrowed("cond_embedding"), cond.into()));
        self.states.append_inputs(&mut inputs)?;
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| inference_error("TSE inference", &e))?;
        let (_, data) = outputs
            .get("extracted_chunk")
            .ok_or_else(|| StageError::Inference("extracted_chunk missing".to_owned()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| inference_error("extracting extracted_chunk", &e))?;
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
