//! UL-UNAS 16 kHz streaming spectral stage.

use std::path::Path;

use ndarray::{Array2, Array4};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};
use ort::{session::Session, value::TensorRef};

use crate::{
    assets::ModelAsset,
    dsp::{StreamingStft, Window},
};

const SAMPLE_RATE: u32 = 16_000;
const FFT_SIZE: usize = 512;
const FRAME_SAMPLES: usize = 256;
const SPECTRUM_BINS: usize = FFT_SIZE / 2 + 1;
const CONV_CACHE: usize = 5_358;
const TFA_CACHE: usize = 402;
const INTER_CACHE: usize = 1_056;

const DESCRIPTOR: StageDescriptor = StageDescriptor {
    id: "ul-unas",
    display_name: "UL-UNAS 16 kHz",
    kind: StageKind::NoiseSuppression,
    sample_rate: SAMPLE_RATE,
    frame_samples: FRAME_SAMPLES,
    algorithmic_delay_samples: FRAME_SAMPLES,
    tail_frames: 1,
    enrollment: EnrollmentRequirement::None,
};

/// Model file required by UL-UNAS.
pub const ASSET: ModelAsset = ModelAsset::UlUnas;

/// Stateful UL-UNAS ONNX stage.
pub struct UlUnas {
    session: Session,
    stft: StreamingStft,
    conv_cache: Array2<f32>,
    tfa_cache: Array2<f32>,
    inter_cache: Array2<f32>,
}

impl UlUnas {
    /// Load the official simplified streaming graph.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if ONNX Runtime rejects the graph.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, StageError> {
        let session = Session::builder()
            .and_then(|builder| builder.with_intra_threads(1))
            .and_then(|builder| builder.with_inter_threads(1))
            .and_then(|builder| builder.commit_from_file(path))
            .map_err(backend_error)?;
        let stft =
            StreamingStft::new(FFT_SIZE, FRAME_SAMPLES, Window::Hann).map_err(backend_error)?;
        Ok(Self {
            session,
            stft,
            conv_cache: Array2::zeros((1, CONV_CACHE)),
            tfa_cache: Array2::zeros((1, TFA_CACHE)),
            inter_cache: Array2::zeros((1, INTER_CACHE)),
        })
    }
}

impl AudioStage for UlUnas {
    fn descriptor(&self) -> StageDescriptor {
        DESCRIPTOR
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(DESCRIPTOR, input, output)?;
        let spectrum = self.stft.analyze(input).map_err(backend_error)?;
        let spectrum =
            Array4::from_shape_vec((1, SPECTRUM_BINS, 1, 2), spectrum).map_err(backend_error)?;
        let (enhanced, conv_cache, tfa_cache, inter_cache) = {
            let outputs = self
                .session
                .run(ort::inputs![
                    "mix" => TensorRef::from_array_view(&spectrum).map_err(backend_error)?,
                    "conv_cache" => TensorRef::from_array_view(&self.conv_cache)
                        .map_err(backend_error)?,
                    "tfa_cache" => TensorRef::from_array_view(&self.tfa_cache)
                        .map_err(backend_error)?,
                    "inter_cache" => TensorRef::from_array_view(&self.inter_cache)
                        .map_err(backend_error)?,
                ])
                .map_err(backend_error)?;
            let enhanced = tensor_values(&outputs, "enh", SPECTRUM_BINS * 2)?;
            let conv_cache = tensor_values(&outputs, "conv_cache_out", CONV_CACHE)?;
            let tfa_cache = tensor_values(&outputs, "tfa_cache_out", TFA_CACHE)?;
            let inter_cache = tensor_values(&outputs, "inter_cache_out", INTER_CACHE)?;
            (enhanced, conv_cache, tfa_cache, inter_cache)
        };
        self.conv_cache =
            Array2::from_shape_vec((1, CONV_CACHE), conv_cache).map_err(backend_error)?;
        self.tfa_cache =
            Array2::from_shape_vec((1, TFA_CACHE), tfa_cache).map_err(backend_error)?;
        self.inter_cache =
            Array2::from_shape_vec((1, INTER_CACHE), inter_cache).map_err(backend_error)?;
        let waveform = self.stft.synthesize(&enhanced).map_err(backend_error)?;
        output.copy_from_slice(&waveform);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.stft.reset();
        self.conv_cache.fill(0.0);
        self.tfa_cache.fill(0.0);
        self.inter_cache.fill(0.0);
        Ok(())
    }
}

fn tensor_values(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &'static str,
    expected: usize,
) -> Result<Vec<f32>, StageError> {
    let (shape, values) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(backend_error)?;
    if values.len() != expected {
        return Err(StageError::Backend {
            stage: DESCRIPTOR.id,
            message: format!(
                "output {name} has shape {shape:?} and {} values, expected {expected}",
                values.len()
            ),
        });
    }
    Ok(values.to_vec())
}

fn backend_error(error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: DESCRIPTOR.id,
        message: error.to_string(),
    }
}
