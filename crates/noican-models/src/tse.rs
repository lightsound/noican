//! Speaker-conditioned 48 kHz streaming Conv-TasNet stage.

use std::{borrow::Cow, path::Path};

use ndarray::{Array2, ArrayD, IxDyn};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};
use ort::{session::Session, value::TensorRef};

use crate::assets::ModelAsset;

/// Width of the ECAPA enrollment vector expected by the TSE graph.
pub const EMBEDDING_DIMENSIONS: usize = 192;
const FRAME_SAMPLES: usize = 480;
const ENCODER_OVERLAP: usize = 48;
const HIDDEN: usize = 256;
const TCN_KERNEL: usize = 3;
const BLOCKS: usize = 6;
const REPEATS: usize = 2;
const STATE_TENSORS: usize = 1 + 3 + 7 * BLOCKS * REPEATS + 1;

const DESCRIPTOR: StageDescriptor = StageDescriptor {
    id: "tse-conv-tasnet-48k",
    display_name: "TSE Conv-TasNet 48 kHz",
    kind: StageKind::SpeakerSuppression,
    sample_rate: 48_000,
    frame_samples: FRAME_SAMPLES,
    algorithmic_delay_samples: ENCODER_OVERLAP,
    tail_frames: 1,
    enrollment: EnrollmentRequirement::SpeakerEmbedding {
        dimensions: EMBEDDING_DIMENSIONS,
    },
};

/// Files required by the TSE graph.
pub const ASSETS: [ModelAsset; 2] = [ModelAsset::TseGraph, ModelAsset::TseWeights];

/// Stateful target-speaker extraction stage.
pub struct Tse {
    session: Session,
    embedding: [f32; EMBEDDING_DIMENSIONS],
    state: Vec<ArrayD<f32>>,
}

impl Tse {
    /// Load the graph and freeze one normalized enrollment embedding.
    ///
    /// The graph's `.onnx.data` sidecar must be next to `path`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if the graph is invalid or
    /// [`StageError::InvalidConfiguration`] for a non-finite or zero vector.
    pub fn load(
        path: impl AsRef<Path>,
        embedding: [f32; EMBEDDING_DIMENSIONS],
    ) -> Result<Self, StageError> {
        validate_embedding(&embedding)?;
        let session = Session::builder()
            .and_then(|builder| builder.with_intra_threads(1))
            .and_then(|builder| builder.with_inter_threads(1))
            .and_then(|builder| builder.commit_from_file(path))
            .map_err(backend_error)?;
        Ok(Self {
            session,
            embedding,
            state: initial_state(),
        })
    }

    /// Replace the enrollment embedding between streams.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::InvalidConfiguration`] for a non-finite or zero
    /// vector.
    pub fn set_embedding(
        &mut self,
        embedding: [f32; EMBEDDING_DIMENSIONS],
    ) -> Result<(), StageError> {
        validate_embedding(&embedding)?;
        self.embedding = embedding;
        self.state = initial_state();
        Ok(())
    }
}

impl AudioStage for Tse {
    fn descriptor(&self) -> StageDescriptor {
        DESCRIPTOR
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(DESCRIPTOR, input, output)?;
        let audio =
            Array2::from_shape_vec((1, FRAME_SAMPLES), input.to_vec()).map_err(backend_error)?;
        let embedding = Array2::from_shape_vec((1, EMBEDDING_DIMENSIONS), self.embedding.to_vec())
            .map_err(backend_error)?;
        let mut inputs: Vec<(Cow<'static, str>, ort::session::SessionInputValue<'_>)> =
            Vec::with_capacity(2 + STATE_TENSORS);
        inputs.push((
            Cow::Borrowed("audio_chunk"),
            TensorRef::from_array_view(&audio)
                .map_err(backend_error)?
                .into(),
        ));
        inputs.push((
            Cow::Borrowed("cond"),
            TensorRef::from_array_view(&embedding)
                .map_err(backend_error)?
                .into(),
        ));
        for (index, state) in self.state.iter().enumerate() {
            inputs.push((
                Cow::Owned(format!("state_in_{index}")),
                TensorRef::from_array_view(state)
                    .map_err(backend_error)?
                    .into(),
            ));
        }
        let (extracted, new_state) = {
            let outputs = self.session.run(inputs).map_err(backend_error)?;
            let (shape, values) = outputs["extracted_chunk"]
                .try_extract_tensor::<f32>()
                .map_err(backend_error)?;
            if shape.as_ref() != [1, FRAME_SAMPLES as i64] {
                return Err(StageError::Backend {
                    stage: DESCRIPTOR.id,
                    message: format!("unexpected extracted_chunk shape {shape:?}"),
                });
            }
            let extracted = values.to_vec();
            let mut new_state = Vec::with_capacity(STATE_TENSORS);
            for index in 0..STATE_TENSORS {
                let name = format!("state_out_{index}");
                let (shape, values) = outputs[name.as_str()]
                    .try_extract_tensor::<f32>()
                    .map_err(backend_error)?;
                let dimensions: Vec<usize> = shape
                    .iter()
                    .map(|dimension| usize::try_from(*dimension))
                    .collect::<Result<_, _>>()
                    .map_err(backend_error)?;
                new_state.push(
                    ArrayD::from_shape_vec(IxDyn(&dimensions), values.to_vec())
                        .map_err(backend_error)?,
                );
            }
            (extracted, new_state)
        };
        output.copy_from_slice(&extracted);
        self.state = new_state;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.state = initial_state();
        Ok(())
    }
}

fn initial_state() -> Vec<ArrayD<f32>> {
    let mut state = Vec::with_capacity(STATE_TENSORS);
    state.push(ArrayD::zeros(IxDyn(&[1, 1, ENCODER_OVERLAP])));
    for _ in 0..3 {
        state.push(ArrayD::zeros(IxDyn(&[1, 1, 1])));
    }
    for _ in 0..REPEATS {
        for block in 0..BLOCKS {
            let dilation = 1_usize << block;
            let padding = (TCN_KERNEL - 1) * dilation;
            state.push(ArrayD::zeros(IxDyn(&[1, HIDDEN, padding])));
            for _ in 0..6 {
                state.push(ArrayD::zeros(IxDyn(&[1, 1, 1])));
            }
        }
    }
    state.push(ArrayD::zeros(IxDyn(&[1, 1, ENCODER_OVERLAP])));
    state
}

fn validate_embedding(embedding: &[f32; EMBEDDING_DIMENSIONS]) -> Result<(), StageError> {
    let squared_norm: f32 = embedding.iter().map(|value| value * value).sum();
    if !squared_norm.is_finite() || squared_norm <= f32::EPSILON {
        return Err(StageError::InvalidConfiguration {
            stage: DESCRIPTOR.id,
            message: "enrollment embedding must be finite and non-zero".to_owned(),
        });
    }
    Ok(())
}

fn backend_error(error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: DESCRIPTOR.id,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_layout_matches_published_contract() {
        let state = initial_state();
        assert_eq!(state.len(), STATE_TENSORS);
        assert_eq!(state[0].shape(), &[1, 1, ENCODER_OVERLAP]);
        assert_eq!(state[STATE_TENSORS - 1].shape(), &[1, 1, ENCODER_OVERLAP]);
    }

    #[test]
    fn zero_embedding_is_rejected() {
        assert!(validate_embedding(&[0.0; EMBEDDING_DIMENSIONS]).is_err());
    }
}
