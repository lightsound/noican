//! Runtime model catalog and stage construction.

use std::{fmt, str::FromStr};

use noican_engine::{AudioStage, RateAdapter, StageError, StageKind};
use thiserror::Error;

use crate::{
    assets::{AssetError, FetchOptions, ModelAsset, ModelStore},
    deep_filter::DeepFilterStage,
    dpdfnet::{DpdfNet, DpdfNetVariant},
    fastenhancer::{FastEnhancer, FastEnhancerVariant},
    tse::{Tse, EMBEDDING_DIMENSIONS},
    ul_unas::UlUnas,
};

/// Every model selectable by the CLI and menu bar UI.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModelId {
    /// `FastEnhancer` Tiny 48 kHz.
    FastEnhancerTiny,
    /// `FastEnhancer` Base 48 kHz.
    FastEnhancerBase,
    /// `FastEnhancer` Small 48 kHz.
    FastEnhancerSmall,
    /// `DPDFNet2` 48 kHz HR.
    DpdfNet2HighResolution,
    /// `DPDFNet8` 48 kHz HR.
    DpdfNet8HighResolution,
    /// Official `DeepFilterNet3` baseline.
    DeepFilterNet3,
    /// UL-UNAS 16 kHz.
    UlUnas,
    /// Hush 16 kHz.
    Hush,
    /// Speaker-conditioned Conv-TasNet 48 kHz.
    TseConvTasNet48k,
}

impl ModelId {
    /// Stable catalog order used by `--all-models` and the menu picker.
    pub const ALL: [Self; 9] = [
        Self::FastEnhancerTiny,
        Self::FastEnhancerBase,
        Self::FastEnhancerSmall,
        Self::DpdfNet2HighResolution,
        Self::DpdfNet8HighResolution,
        Self::DeepFilterNet3,
        Self::UlUnas,
        Self::Hush,
        Self::TseConvTasNet48k,
    ];

    /// Stable machine-readable identifier.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::FastEnhancerTiny => "fastenhancer-t",
            Self::FastEnhancerBase => "fastenhancer-b",
            Self::FastEnhancerSmall => "fastenhancer-s",
            Self::DpdfNet2HighResolution => "dpdfnet2-48khz-hr",
            Self::DpdfNet8HighResolution => "dpdfnet8-48khz-hr",
            Self::DeepFilterNet3 => "deepfilternet3",
            Self::UlUnas => "ul-unas",
            Self::Hush => "hush",
            Self::TseConvTasNet48k => "tse-conv-tasnet-48k",
        }
    }

    /// Human-readable picker label.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::FastEnhancerTiny => "FastEnhancer Tiny 48 kHz",
            Self::FastEnhancerBase => "FastEnhancer Base 48 kHz",
            Self::FastEnhancerSmall => "FastEnhancer Small 48 kHz",
            Self::DpdfNet2HighResolution => "DPDFNet2 48 kHz HR",
            Self::DpdfNet8HighResolution => "DPDFNet8 48 kHz HR",
            Self::DeepFilterNet3 => "DeepFilterNet3",
            Self::UlUnas => "UL-UNAS 16 kHz",
            Self::Hush => "Hush 16 kHz",
            Self::TseConvTasNet48k => "TSE Conv-TasNet 48 kHz",
        }
    }

    /// Model purpose.
    #[must_use]
    pub const fn kind(self) -> StageKind {
        match self {
            Self::Hush | Self::TseConvTasNet48k => StageKind::SpeakerSuppression,
            Self::FastEnhancerTiny
            | Self::FastEnhancerBase
            | Self::FastEnhancerSmall
            | Self::DpdfNet2HighResolution
            | Self::DpdfNet8HighResolution
            | Self::DeepFilterNet3
            | Self::UlUnas => StageKind::NoiseSuppression,
        }
    }

    /// Native model sample rate.
    #[must_use]
    pub const fn native_sample_rate(self) -> u32 {
        match self {
            Self::UlUnas | Self::Hush => 16_000,
            Self::FastEnhancerTiny
            | Self::FastEnhancerBase
            | Self::FastEnhancerSmall
            | Self::DpdfNet2HighResolution
            | Self::DpdfNet8HighResolution
            | Self::DeepFilterNet3
            | Self::TseConvTasNet48k => 48_000,
        }
    }

    /// Whether a 192-dimensional ECAPA enrollment is required.
    #[must_use]
    pub const fn requires_enrollment(self) -> bool {
        matches!(self, Self::TseConvTasNet48k)
    }

    /// Files fetched by this model.
    #[must_use]
    pub const fn assets(self) -> &'static [ModelAsset] {
        match self {
            Self::FastEnhancerTiny => &[ModelAsset::FastEnhancerTiny],
            Self::FastEnhancerBase => &[ModelAsset::FastEnhancerBase],
            Self::FastEnhancerSmall => &[ModelAsset::FastEnhancerSmall],
            Self::DpdfNet2HighResolution => &[ModelAsset::DpdfNet2HighResolution],
            Self::DpdfNet8HighResolution => &[ModelAsset::DpdfNet8HighResolution],
            Self::DeepFilterNet3 => &[],
            Self::UlUnas => &[ModelAsset::UlUnas],
            Self::Hush => &[ModelAsset::Hush],
            Self::TseConvTasNet48k => &[ModelAsset::TseGraph, ModelAsset::TseWeights],
        }
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.slug())
    }
}

impl FromStr for ModelId {
    type Err = UnknownModel;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|model| model.slug() == value)
            .ok_or_else(|| UnknownModel(value.to_owned()))
    }
}

/// Unknown model slug.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("unknown model {0:?}")]
pub struct UnknownModel(String);

/// Inputs needed to construct one model.
pub struct LoadRequest<'a> {
    /// Requested model.
    pub model: ModelId,
    /// Verified model cache.
    pub store: &'a ModelStore,
    /// Optional access token for gated Hugging Face files.
    pub hugging_face_token: Option<&'a str>,
    /// Required only for TSE.
    pub speaker_embedding: Option<[f32; EMBEDDING_DIMENSIONS]>,
}

/// Download assets and construct a model normalized to the pipeline contract.
///
/// # Errors
///
/// Returns [`ModelLoadError`] for assets, enrollment, or backend failures.
pub fn load_pipeline_stage(
    request: &LoadRequest<'_>,
) -> Result<Box<dyn AudioStage>, ModelLoadError> {
    let native = load_native_stage(request)?;
    Ok(Box::new(RateAdapter::new(native)?))
}

fn load_native_stage(request: &LoadRequest<'_>) -> Result<Box<dyn AudioStage>, ModelLoadError> {
    let fetch = FetchOptions {
        hugging_face_token: request.hugging_face_token,
    };
    let stage: Box<dyn AudioStage> = match request.model {
        ModelId::FastEnhancerTiny => Box::new(FastEnhancer::load(
            request.store.ensure(ModelAsset::FastEnhancerTiny, fetch)?,
            FastEnhancerVariant::Tiny,
        )?),
        ModelId::FastEnhancerBase => Box::new(FastEnhancer::load(
            request.store.ensure(ModelAsset::FastEnhancerBase, fetch)?,
            FastEnhancerVariant::Base,
        )?),
        ModelId::FastEnhancerSmall => Box::new(FastEnhancer::load(
            request.store.ensure(ModelAsset::FastEnhancerSmall, fetch)?,
            FastEnhancerVariant::Small,
        )?),
        ModelId::DpdfNet2HighResolution => Box::new(DpdfNet::load(
            request
                .store
                .ensure(ModelAsset::DpdfNet2HighResolution, fetch)?,
            DpdfNetVariant::DpdfNet2,
        )?),
        ModelId::DpdfNet8HighResolution => Box::new(DpdfNet::load(
            request
                .store
                .ensure(ModelAsset::DpdfNet8HighResolution, fetch)?,
            DpdfNetVariant::DpdfNet8,
        )?),
        ModelId::DeepFilterNet3 => Box::new(DeepFilterStage::deep_filter_net3()?),
        ModelId::UlUnas => Box::new(UlUnas::load(
            request.store.ensure(ModelAsset::UlUnas, fetch)?,
        )?),
        ModelId::Hush => Box::new(DeepFilterStage::hush(
            request.store.ensure(ModelAsset::Hush, fetch)?,
        )?),
        ModelId::TseConvTasNet48k => {
            let graph = request.store.ensure(ModelAsset::TseGraph, fetch)?;
            request.store.ensure(ModelAsset::TseWeights, fetch)?;
            let embedding = request
                .speaker_embedding
                .ok_or(ModelLoadError::MissingEnrollment)?;
            Box::new(Tse::load(graph, &embedding)?)
        }
    };
    Ok(stage)
}

/// Model preparation failures.
#[derive(Debug, Error)]
pub enum ModelLoadError {
    /// A required file could not be fetched or verified.
    #[error(transparent)]
    Asset(#[from] AssetError),
    /// Model construction or adaptation failed.
    #[error(transparent)]
    Stage(#[from] StageError),
    /// TSE was requested without a speaker embedding.
    #[error("tse-conv-tasnet-48k requires a 192-dimensional ECAPA enrollment embedding")]
    MissingEnrollment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_round_trip() -> Result<(), UnknownModel> {
        for model in ModelId::ALL {
            assert_eq!(model.slug().parse::<ModelId>()?, model);
        }
        Ok(())
    }

    #[test]
    fn only_tse_requires_enrollment() {
        let enrolled: Vec<ModelId> = ModelId::ALL
            .into_iter()
            .filter(|model| model.requires_enrollment())
            .collect();
        assert_eq!(enrolled, [ModelId::TseConvTasNet48k]);
    }
}
