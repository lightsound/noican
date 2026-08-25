//! DeepFilterNet3 baseline and Hush stages using the upstream Rust runtime.

use std::path::{Path, PathBuf};

use df::tract::{DfParams, DfTract, ReduceMask, RuntimeParams};
use ndarray015::{ArrayView2, ArrayViewMut2};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};

use crate::assets::ModelAsset;

/// DeepFilterNet-family model exposed by this backend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeepFilterVariant {
    /// Official upstream DeepFilterNet3 model embedded by `libDF`.
    DeepFilterNet3,
    /// Weya Hush 16 kHz background-speaker suppression model.
    Hush,
}

impl DeepFilterVariant {
    /// External model file, if this variant does not use embedded weights.
    #[must_use]
    pub const fn asset(self) -> Option<ModelAsset> {
        match self {
            Self::DeepFilterNet3 => None,
            Self::Hush => Some(ModelAsset::Hush),
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::DeepFilterNet3 => "deepfilternet3",
            Self::Hush => "hush",
        }
    }

    const fn display_name(self) -> &'static str {
        match self {
            Self::DeepFilterNet3 => "DeepFilterNet3",
            Self::Hush => "Hush 16 kHz",
        }
    }

    const fn kind(self) -> StageKind {
        match self {
            Self::DeepFilterNet3 => StageKind::NoiseSuppression,
            Self::Hush => StageKind::SpeakerSuppression,
        }
    }
}

#[derive(Clone, Debug)]
enum ModelSource {
    Embedded,
    Bundle(PathBuf),
}

/// Stateful DeepFilterNet-family stage.
pub struct DeepFilterStage {
    variant: DeepFilterVariant,
    source: ModelSource,
    descriptor: StageDescriptor,
    model: DfTract,
}

impl DeepFilterStage {
    /// Load the official embedded DeepFilterNet3 baseline.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if the embedded model cannot compile.
    pub fn deep_filter_net3() -> Result<Self, StageError> {
        Self::build(DeepFilterVariant::DeepFilterNet3, ModelSource::Embedded)
    }

    /// Load Hush from its official ONNX tar bundle.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if the bundle is missing, malformed, or
    /// rejected by the DeepFilterNet runtime.
    pub fn hush(path: impl AsRef<Path>) -> Result<Self, StageError> {
        Self::build(
            DeepFilterVariant::Hush,
            ModelSource::Bundle(path.as_ref().to_path_buf()),
        )
    }

    fn build(variant: DeepFilterVariant, source: ModelSource) -> Result<Self, StageError> {
        let params = load_params(variant, &source)?;
        let runtime = runtime_params(variant);
        let model =
            DfTract::new(params, &runtime).map_err(|error| backend_error(variant, error))?;
        let sample_rate = u32::try_from(model.sr).map_err(|error| backend_error(variant, error))?;
        let algorithmic_delay_samples = model
            .lookahead
            .checked_add(1)
            .and_then(|frames| frames.checked_mul(model.hop_size))
            .ok_or_else(|| StageError::InvalidConfiguration {
                stage: variant.id(),
                message: "DeepFilterNet latency overflow".to_owned(),
            })?;
        let descriptor = StageDescriptor {
            id: variant.id(),
            display_name: variant.display_name(),
            kind: variant.kind(),
            sample_rate,
            frame_samples: model.hop_size,
            algorithmic_delay_samples,
            tail_frames: model.lookahead + 1,
            enrollment: EnrollmentRequirement::None,
        };
        Ok(Self {
            variant,
            source,
            descriptor,
            model,
        })
    }
}

impl AudioStage for DeepFilterStage {
    fn descriptor(&self) -> StageDescriptor {
        self.descriptor
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(self.descriptor, input, output)?;
        let input = ArrayView2::from_shape((1, input.len()), input)
            .map_err(|error| backend_error(self.variant, error))?;
        let output = ArrayViewMut2::from_shape((1, output.len()), output)
            .map_err(|error| backend_error(self.variant, error))?;
        self.model
            .process(input, output)
            .map_err(|error| backend_error(self.variant, error))?;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        let params = load_params(self.variant, &self.source)?;
        self.model = DfTract::new(params, &runtime_params(self.variant))
            .map_err(|error| backend_error(self.variant, error))?;
        Ok(())
    }
}

fn load_params(variant: DeepFilterVariant, source: &ModelSource) -> Result<DfParams, StageError> {
    match source {
        ModelSource::Embedded => Ok(DfParams::default()),
        ModelSource::Bundle(path) => {
            DfParams::new(path.clone()).map_err(|error| backend_error(variant, error))
        }
    }
}

fn runtime_params(variant: DeepFilterVariant) -> RuntimeParams {
    match variant {
        DeepFilterVariant::DeepFilterNet3 => RuntimeParams::default_with_ch(1),
        DeepFilterVariant::Hush => {
            RuntimeParams::new(1, 0.0, 100.0, -15.0, 35.0, 35.0, ReduceMask::MAX)
        }
    }
}

fn backend_error(variant: DeepFilterVariant, error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: variant.id(),
        message: error.to_string(),
    }
}
