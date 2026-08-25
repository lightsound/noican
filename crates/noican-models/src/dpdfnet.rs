//! `DPDFNet` 48 kHz HR streaming stages.

use std::path::Path;

use ndarray::{Array1, Array4};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};
use ort::{session::Session, value::TensorRef};

use crate::{
    assets::ModelAsset,
    dsp::{StreamingStft, Window},
};

const SAMPLE_RATE: u32 = 48_000;
const FFT_SIZE: usize = 960;
const FRAME_SAMPLES: usize = 480;
const SPECTRUM_BINS: usize = FFT_SIZE / 2 + 1;

/// Official high-resolution `DPDFNet` variants.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DpdfNetVariant {
    /// Balanced `DPDFNet2` model.
    DpdfNet2,
    /// Higher-quality `DPDFNet8` model.
    DpdfNet8,
}

impl DpdfNetVariant {
    /// Model file required by this variant.
    #[must_use]
    pub const fn asset(self) -> ModelAsset {
        match self {
            Self::DpdfNet2 => ModelAsset::DpdfNet2HighResolution,
            Self::DpdfNet8 => ModelAsset::DpdfNet8HighResolution,
        }
    }

    const fn metadata_profile(self) -> &'static str {
        match self {
            Self::DpdfNet2 => "dpdfnet2_48khz_hr",
            // The official Ceva-IP dpdfnet8_48khz_hr file at revision
            // dd6818d incorrectly retains the DPDFNet2 profile string. Its
            // larger state tensor and pinned file digest identify the graph.
            Self::DpdfNet8 => "dpdfnet2_48khz_hr",
        }
    }

    const fn expected_state_size(self) -> usize {
        match self {
            Self::DpdfNet2 => 56_436,
            Self::DpdfNet8 => 90_228,
        }
    }

    const fn descriptor(self) -> StageDescriptor {
        let (id, display_name) = match self {
            Self::DpdfNet2 => ("dpdfnet2-48khz-hr", "DPDFNet2 48 kHz HR"),
            Self::DpdfNet8 => ("dpdfnet8-48khz-hr", "DPDFNet8 48 kHz HR"),
        };
        StageDescriptor {
            id,
            display_name,
            kind: StageKind::NoiseSuppression,
            sample_rate: SAMPLE_RATE,
            frame_samples: FRAME_SAMPLES,
            algorithmic_delay_samples: FRAME_SAMPLES,
            tail_frames: 1,
            enrollment: EnrollmentRequirement::None,
        }
    }
}

/// Stateful `DPDFNet` spectral ONNX stage.
pub struct DpdfNet {
    variant: DpdfNetVariant,
    session: Session,
    stft: StreamingStft,
    initial_state: Vec<f32>,
    state: Vec<f32>,
}

impl DpdfNet {
    /// Load an official `DPDFNet` 48 kHz HR graph and its embedded metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] for a malformed or incompatible graph.
    pub fn load(path: impl AsRef<Path>, variant: DpdfNetVariant) -> Result<Self, StageError> {
        let descriptor = variant.descriptor();
        let session = Session::builder()
            .map_err(|error| backend_error(descriptor, error))?
            .with_intra_threads(1)
            .map_err(|error| backend_error(descriptor, error))?
            .with_inter_threads(1)
            .map_err(|error| backend_error(descriptor, error))?
            .commit_from_file(path)
            .map_err(|error| backend_error(descriptor, error))?;
        validate_metadata(&session, variant)?;
        let state_size = metadata_usize(&session, descriptor, "state_size")?;
        if state_size != variant.expected_state_size() {
            return Err(StageError::Backend {
                stage: descriptor.id,
                message: format!(
                    "metadata state_size={state_size}, expected {}",
                    variant.expected_state_size()
                ),
            });
        }
        let erb_state_size = metadata_usize(&session, descriptor, "erb_norm_state_size")?;
        let spec_state_size = metadata_usize(&session, descriptor, "spec_norm_state_size")?;
        let erb = metadata_floats(&session, descriptor, "erb_norm_init")?;
        let spec = metadata_floats(&session, descriptor, "spec_norm_init")?;
        if erb.len() != erb_state_size
            || spec.len() != spec_state_size
            || erb_state_size
                .checked_add(spec_state_size)
                .is_none_or(|prefix| prefix > state_size)
        {
            return Err(StageError::Backend {
                stage: descriptor.id,
                message: "normalization state metadata is inconsistent".to_owned(),
            });
        }
        let mut initial_state = vec![0.0_f32; state_size];
        initial_state[..erb.len()].copy_from_slice(&erb);
        initial_state[erb_state_size..erb_state_size + spec.len()].copy_from_slice(&spec);
        let stft = StreamingStft::new(FFT_SIZE, FRAME_SAMPLES, Window::Vorbis)
            .map_err(|error| backend_error(descriptor, error))?;
        Ok(Self {
            variant,
            session,
            stft,
            state: initial_state.clone(),
            initial_state,
        })
    }
}

impl AudioStage for DpdfNet {
    fn descriptor(&self) -> StageDescriptor {
        self.variant.descriptor()
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        let descriptor = self.descriptor();
        validate_frame_lengths(descriptor, input, output)?;
        let spectrum = self
            .stft
            .analyze(input)
            .map_err(|error| backend_error(descriptor, error))?;
        let spectrum = Array4::from_shape_vec((1, 1, SPECTRUM_BINS, 2), spectrum)
            .map_err(|error| backend_error(descriptor, error))?;
        let state = Array1::from_vec(self.state.clone());
        let (enhanced, state) = {
            let outputs = self
                .session
                .run(ort::inputs![
                    "spec" => TensorRef::from_array_view(&spectrum)
                        .map_err(|error| backend_error(descriptor, error))?,
                    "state_in" => TensorRef::from_array_view(&state)
                        .map_err(|error| backend_error(descriptor, error))?,
                ])
                .map_err(|error| backend_error(descriptor, error))?;
            let (shape, values) = outputs["spec_e"]
                .try_extract_tensor::<f32>()
                .map_err(|error| backend_error(descriptor, error))?;
            if shape.as_ref() != [1, 1, 481, 2] {
                return Err(StageError::Backend {
                    stage: descriptor.id,
                    message: format!("unexpected spec_e shape {shape:?}"),
                });
            }
            let enhanced = values.to_vec();
            let (_, state) = outputs["state_out"]
                .try_extract_tensor::<f32>()
                .map_err(|error| backend_error(descriptor, error))?;
            (enhanced, state.to_vec())
        };
        if state.len() != self.initial_state.len() {
            return Err(StageError::Backend {
                stage: descriptor.id,
                message: format!(
                    "state_out contains {} values, expected {}",
                    state.len(),
                    self.initial_state.len()
                ),
            });
        }
        let waveform = self
            .stft
            .synthesize(&enhanced)
            .map_err(|error| backend_error(descriptor, error))?;
        output.copy_from_slice(&waveform);
        self.state = state;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.state.clone_from(&self.initial_state);
        self.stft.reset();
        Ok(())
    }
}

fn validate_metadata(session: &Session, variant: DpdfNetVariant) -> Result<(), StageError> {
    let descriptor = variant.descriptor();
    for (key, expected) in [
        ("model_type", "dpdfnet"),
        ("profile", variant.metadata_profile()),
        ("sample_rate", "48000"),
        ("n_fft", "960"),
        ("hop_length", "480"),
        ("window_type", "vorbis"),
    ] {
        let actual = metadata_string(session, descriptor, key)?;
        if actual != expected {
            return Err(StageError::Backend {
                stage: descriptor.id,
                message: format!("metadata {key}={actual:?}, expected {expected:?}"),
            });
        }
    }
    Ok(())
}

fn metadata_string(
    session: &Session,
    descriptor: StageDescriptor,
    key: &'static str,
) -> Result<String, StageError> {
    session
        .metadata()
        .map_err(|error| backend_error(descriptor, error))?
        .custom(key)
        .ok_or_else(|| StageError::Backend {
            stage: descriptor.id,
            message: format!("missing ONNX metadata key {key}"),
        })
}

fn metadata_usize(
    session: &Session,
    descriptor: StageDescriptor,
    key: &'static str,
) -> Result<usize, StageError> {
    metadata_string(session, descriptor, key)?
        .parse()
        .map_err(|error| backend_error(descriptor, error))
}

fn metadata_floats(
    session: &Session,
    descriptor: StageDescriptor,
    key: &'static str,
) -> Result<Vec<f32>, StageError> {
    metadata_string(session, descriptor, key)?
        .split(',')
        .map(|value| {
            value
                .parse()
                .map_err(|error| backend_error(descriptor, error))
        })
        .collect()
}

fn backend_error(descriptor: StageDescriptor, error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: descriptor.id,
        message: error.to_string(),
    }
}
