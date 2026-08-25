//! `FastEnhancer` 48 kHz T/B/S streaming ONNX stages.

use std::{borrow::Cow, path::Path};

use ndarray::{Array2, ArrayD, IxDyn};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};
use ort::{session::Session, value::TensorRef};

use crate::assets::ModelAsset;

const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 512;

/// Supported `FastEnhancer` 48 kHz variants.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FastEnhancerVariant {
    /// 28K-parameter Tiny model.
    Tiny,
    /// 101K-parameter Base model.
    Base,
    /// 207K-parameter Small model.
    Small,
}

impl FastEnhancerVariant {
    /// Model file required by this variant.
    #[must_use]
    pub const fn asset(self) -> ModelAsset {
        match self {
            Self::Tiny => ModelAsset::FastEnhancerTiny,
            Self::Base => ModelAsset::FastEnhancerBase,
            Self::Small => ModelAsset::FastEnhancerSmall,
        }
    }

    const fn descriptor(self) -> StageDescriptor {
        let (id, display_name) = match self {
            Self::Tiny => ("fastenhancer-t", "FastEnhancer Tiny 48 kHz"),
            Self::Base => ("fastenhancer-b", "FastEnhancer Base 48 kHz"),
            Self::Small => ("fastenhancer-s", "FastEnhancer Small 48 kHz"),
        };
        StageDescriptor {
            id,
            display_name,
            kind: StageKind::NoiseSuppression,
            sample_rate: SAMPLE_RATE,
            frame_samples: FRAME_SAMPLES,
            algorithmic_delay_samples: FRAME_SAMPLES,
            tail_frames: 2,
            enrollment: EnrollmentRequirement::None,
        }
    }

    const fn cache_shapes(self) -> &'static [&'static [usize]] {
        const TINY: &[&[usize]] = &[&[1, 512], &[1, 512], &[1, 24, 20], &[1, 24, 20]];
        const BASE: &[&[usize]] = &[
            &[1, 512],
            &[1, 512],
            &[1, 36, 36],
            &[1, 36, 36],
            &[1, 36, 36],
        ];
        const SMALL: &[&[usize]] = &[
            &[1, 512],
            &[1, 512],
            &[1, 48, 48],
            &[1, 48, 48],
            &[1, 48, 48],
        ];
        match self {
            Self::Tiny => TINY,
            Self::Base => BASE,
            Self::Small => SMALL,
        }
    }
}

/// Stateful `FastEnhancer` ONNX stage.
pub struct FastEnhancer {
    variant: FastEnhancerVariant,
    session: Session,
    caches: Vec<ArrayD<f32>>,
}

impl FastEnhancer {
    /// Load one official 48 kHz model graph.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] if ONNX Runtime rejects the graph.
    pub fn load(path: impl AsRef<Path>, variant: FastEnhancerVariant) -> Result<Self, StageError> {
        let descriptor = variant.descriptor();
        let session = Session::builder()
            .map_err(|error| backend_error(descriptor, error))?
            .with_intra_threads(1)
            .map_err(|error| backend_error(descriptor, error))?
            .with_inter_threads(1)
            .map_err(|error| backend_error(descriptor, error))?
            .commit_from_file(path)
            .map_err(|error| backend_error(descriptor, error))?;
        let caches = make_caches(variant);
        Ok(Self {
            variant,
            session,
            caches,
        })
    }
}

impl AudioStage for FastEnhancer {
    fn descriptor(&self) -> StageDescriptor {
        self.variant.descriptor()
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        let descriptor = self.descriptor();
        validate_frame_lengths(descriptor, input, output)?;
        let waveform = Array2::from_shape_vec((1, FRAME_SAMPLES), input.to_vec())
            .map_err(|error| backend_error(descriptor, error))?;
        let mut inputs: Vec<(Cow<'static, str>, ort::session::SessionInputValue<'_>)> =
            Vec::with_capacity(1 + self.caches.len());
        inputs.push((
            Cow::Borrowed("wav_in"),
            TensorRef::from_array_view(&waveform)
                .map_err(|error| backend_error(descriptor, error))?
                .into(),
        ));
        for (index, cache) in self.caches.iter().enumerate() {
            inputs.push((
                Cow::Owned(format!("cache_in_{index}")),
                TensorRef::from_array_view(cache)
                    .map_err(|error| backend_error(descriptor, error))?
                    .into(),
            ));
        }
        let (waveform_output, caches) = {
            let outputs = self
                .session
                .run(inputs)
                .map_err(|error| backend_error(descriptor, error))?;
            let (shape, values) = outputs["wav_out"]
                .try_extract_tensor::<f32>()
                .map_err(|error| backend_error(descriptor, error))?;
            if shape.as_ref() != [1, 512] {
                return Err(StageError::Backend {
                    stage: descriptor.id,
                    message: format!("unexpected wav_out shape {shape:?}"),
                });
            }
            let waveform_output = values.to_vec();
            let mut caches = Vec::with_capacity(self.caches.len());
            for index in 0..self.caches.len() {
                let name = format!("cache_out_{index}");
                let (shape, values) = outputs[name.as_str()]
                    .try_extract_tensor::<f32>()
                    .map_err(|error| backend_error(descriptor, error))?;
                let dimensions: Vec<usize> = shape
                    .iter()
                    .map(|dimension| usize::try_from(*dimension))
                    .collect::<Result<_, _>>()
                    .map_err(|error| backend_error(descriptor, error))?;
                let cache = ArrayD::from_shape_vec(IxDyn(&dimensions), values.to_vec())
                    .map_err(|error| backend_error(descriptor, error))?;
                caches.push(cache);
            }
            (waveform_output, caches)
        };
        output.copy_from_slice(&waveform_output);
        self.caches = caches;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        for cache in &mut self.caches {
            cache.fill(0.0);
        }
        Ok(())
    }
}

fn make_caches(variant: FastEnhancerVariant) -> Vec<ArrayD<f32>> {
    variant
        .cache_shapes()
        .iter()
        .map(|shape| ArrayD::zeros(IxDyn(shape)))
        .collect()
}

fn backend_error(descriptor: StageDescriptor, error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: descriptor.id,
        message: error.to_string(),
    }
}
