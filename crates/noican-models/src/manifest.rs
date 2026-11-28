//! See docs/adr/0009-model-catalog-fetch.md#model-weight-pinning.

use std::path::{Path, PathBuf};

use noican_core::{FrameProcessor, FramedStage, Stage, StageError};

use crate::factory::MAX_BLOCK_LEN;
use crate::fetch::model_dir;
use crate::stages::dfn_tract::DfTractStage;
use crate::stages::dpdfnet::DpdfnetStage;
use crate::stages::hush_wideband::HushWidebandStage;
use crate::stages::ulunas::UlunasStage;
use crate::traits::ModelTraits;

pub(crate) type StageBuilder = fn(&ModelSpec, &Path) -> Result<Box<dyn Stage>, StageError>;

/// Broad role of a model in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Suppresses non-speech noise.
    Denoise,
    /// Suppresses background speakers (keeps the target speaker).
    SpeakerSuppression,
}

/// One downloadable file belonging to a model.
#[derive(Debug, Clone, Copy)]
pub struct FileSpec {
    /// File name under the model's directory.
    pub name: &'static str,
    /// Direct download URL (official release asset).
    pub url: &'static str,
    /// See docs/adr/0009-model-catalog-fetch.md#model-weight-pinning.
    pub sha256: Option<&'static str>,
}

/// A model available to the engine and CLI.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    /// See docs/adr/0009-model-catalog-fetch.md#model-id-stability.
    pub id: &'static str,
    /// Human-readable name for UIs.
    pub display_name: &'static str,
    /// Pipeline role.
    pub family: ModelFamily,
    /// Native processing rate (Hz).
    pub sample_rate: u32,
    /// License of the weights (informational; see `THIRD_PARTY_NOTICES.md`).
    pub license: &'static str,
    /// See docs/adr/0009-model-catalog-fetch.md#model-depends-on.
    pub files: &'static [FileSpec],
    /// See docs/adr/0009-model-catalog-fetch.md#model-depends-on.
    pub depends_on: &'static [&'static str],
    pub(crate) traits: ModelTraits,
    pub(crate) build: StageBuilder,
}

impl ModelSpec {
    /// Looks a model up by [`ModelSpec::id`].
    #[must_use]
    pub fn find(id: &str) -> Option<&'static Self> {
        ALL_MODELS.iter().find(|m| m.id == id)
    }

    /// See docs/adr/0009-model-catalog-fetch.md#model-depends-on.
    pub fn dependencies(&self) -> impl Iterator<Item = &'static Self> {
        self.depends_on.iter().filter_map(|id| Self::find(id))
    }

    fn file_path(&self, models_dir: &Path, index: usize) -> PathBuf {
        model_dir(models_dir, self).join(self.files[index].name)
    }

    fn dependency_file_path(
        &self,
        models_dir: &Path,
        dependency: &str,
        index: usize,
    ) -> Result<PathBuf, StageError> {
        let dep = self
            .dependencies()
            .find(|dep| dep.id == dependency)
            .ok_or_else(|| {
                StageError::Unsupported(format!(
                    "{} does not depend on {dependency} (registry inconsistency)",
                    self.id
                ))
            })?;
        Ok(dep.file_path(models_dir, index))
    }
}

fn framed<P: FrameProcessor + 'static>(processor: P) -> Result<Box<dyn Stage>, StageError> {
    Ok(Box::new(FramedStage::new(processor, MAX_BLOCK_LEN)?))
}

/// All models known to this build.
pub static ALL_MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "dpdfnet2",
        display_name: "DPDFNet2 48k HR",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "Apache-2.0",
        files: &[FileSpec {
            name: "dpdfnet2_48khz_hr.onnx",
            url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/dpdfnet2_48khz_hr.onnx",
            sha256: Some("0b399f8a58dc4d70d8cd97541f5c39869406145193b957d00a03b66070944928"),
        }],
        depends_on: &[],
        traits: ModelTraits::rated(
            [4, 4, 2, 2],
            "strong cleanup",
            "48 kHz native, ~60 ms delay, ~2.6M parameters.",
        ),
        build: |spec, dir| framed(DpdfnetStage::new(spec.id, &spec.file_path(dir, 0))?),
    },
    ModelSpec {
        id: "dpdfnet8",
        display_name: "DPDFNet8 48k HR",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "Apache-2.0",
        files: &[FileSpec {
            name: "dpdfnet8_48khz_hr.onnx",
            url: "https://huggingface.co/Ceva-IP/DPDFNet/resolve/main/onnx/dpdfnet8_48khz_hr.onnx",
            sha256: Some("7b3afbb260a08fe9af3d16e3bda992971be1e7e951d1dee7c2d235f5c43f5631"),
        }],
        depends_on: &[],
        traits: ModelTraits::rated(
            [5, 4, 2, 2],
            "strongest cleanup",
            "48 kHz native, ~60 ms delay, ~3.7M parameters. \
             Suppresses keyboard/trackpad clicks.",
        ),
        build: |spec, dir| framed(DpdfnetStage::new(spec.id, &spec.file_path(dir, 0))?),
    },
    ModelSpec {
        id: "dfn3",
        display_name: "DeepFilterNet3 48k",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "MIT OR Apache-2.0",
        files: &[],
        depends_on: &[],
        traits: ModelTraits::rated(
            [5, 4, 3, 3],
            "strong cleanup, moderate delay",
            "48 kHz native, ~40 ms delay, ~2.1M parameters. \
             Suppresses keyboard/trackpad clicks.",
        ),
        build: |spec, _| framed(DfTractStage::deepfilternet3(spec.id)?),
    },
    ModelSpec {
        id: "dfn3-ll",
        display_name: "DeepFilterNet3 LL 48k",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "MIT OR Apache-2.0",
        files: &[FileSpec {
            name: "DeepFilterNet3_ll_onnx.tar.gz",
            url: "https://raw.githubusercontent.com/Rikorose/DeepFilterNet/978576aa8400552a4ce9730838c635aa30db5e61/models/DeepFilterNet3_ll_onnx.tar.gz",
            sha256: Some("5998e58e8ba0e09bb76986ef97b84afa065a571ef282d4a1222f341e3251cf3a"),
        }],
        depends_on: &[],
        traits: ModelTraits::rated(
            [4, 4, 5, 2],
            "strong cleanup, low delay",
            "48 kHz native, ~20 ms delay, ~9.7M parameters. \
             DeepFilterNet3 variant without lookahead.",
        ),
        build: |spec, dir| {
            framed(DfTractStage::deepfilternet3_ll(
                spec.id,
                &spec.file_path(dir, 0),
            )?)
        },
    },
    ModelSpec {
        id: "ul-unas",
        display_name: "UL-UNAS 16k",
        family: ModelFamily::Denoise,
        sample_rate: 16_000,
        license: "MIT",
        files: &[FileSpec {
            name: "ulunas_stream_simple.onnx",
            url: "https://raw.githubusercontent.com/Xiaobin-Rong/ul-unas/00f7c700da43d38347f30a6ccebd86fcbc798e07/ulunas_onnx/onnx_models/ulunas_stream_simple.onnx",
            sha256: Some("f2e804d54d6a88f4f82f44d86c9f1cf646db2509bfca935cfbfc5fcd8cbfac3b"),
        }],
        depends_on: &[],
        traits: ModelTraits::rated(
            [3, 2, 4, 5],
            "light, phone-quality voice",
            "16 kHz model (telephony-band voice), ~35 ms delay, \
             ~0.2M parameters.",
        ),
        build: |spec, dir| framed(UlunasStage::new(spec.id, &spec.file_path(dir, 0))?),
    },
    ModelSpec {
        id: "hush",
        display_name: "Hush 16k",
        family: ModelFamily::SpeakerSuppression,
        sample_rate: 16_000,
        license: "Apache-2.0",
        files: &[FileSpec {
            name: "advanced_dfnet16k_model_best_onnx.tar.gz",
            url: "https://huggingface.co/weya-ai/hush/resolve/main/onnx/advanced_dfnet16k_model_best_onnx.tar.gz",
            sha256: Some("45632ccaa82b71bb743d6caa7c78e983fe2f2790a3af7f6ec48e6ed7ba085df6"),
        }],
        depends_on: &[],
        traits: ModelTraits::rated(
            [4, 2, 5, 3],
            "mutes background voices",
            "16 kHz model (telephony-band voice), ~23 ms delay, \
             ~2.2M parameters. Also suppresses other people talking \
             nearby and keyboard/trackpad clicks.",
        ),
        build: |spec, dir| framed(DfTractStage::hush(spec.id, &spec.file_path(dir, 0))?),
    },
    ModelSpec {
        id: "hush-48k",
        display_name: "Hush 48k",
        family: ModelFamily::SpeakerSuppression,
        sample_rate: 48_000,
        license: "Apache-2.0",
        files: &[],
        depends_on: &["hush"],
        traits: ModelTraits::rated(
            [4, 4, 5, 3],
            "mutes background voices, full-band voice",
            "48 kHz output: Hush's suppression with your voice's upper \
             band kept (gated by Hush itself), ~23 ms delay, ~2.2M \
             parameters. Also suppresses other people talking nearby \
             and keyboard/trackpad clicks.",
        ),
        build: |spec, dir| {
            let tarball = spec.dependency_file_path(dir, "hush", 0)?;
            framed(HushWidebandStage::new(spec.id, &tarball)?)
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = ALL_MODELS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL_MODELS.len(), "duplicate model id");
    }

    #[test]
    fn dependencies_resolve_and_are_one_level_deep() {
        for model in ALL_MODELS {
            assert_eq!(
                model.dependencies().count(),
                model.depends_on.len(),
                "{}: a depends_on id is not in the registry",
                model.id
            );
            for dep in model.dependencies() {
                assert!(
                    dep.depends_on.is_empty(),
                    "{}: dependency {} has dependencies of its own",
                    model.id,
                    dep.id
                );
                assert!(
                    !dep.files.is_empty(),
                    "{}: dependency {} has no files, so depending on it is pointless",
                    model.id,
                    dep.id
                );
            }
        }
    }

    #[test]
    fn hush_48k_reuses_the_hush_weights() {
        let spec = ModelSpec::find("hush-48k").expect("registered");
        assert_eq!(spec.sample_rate, 48_000);
        assert!(spec.files.is_empty());
        assert_eq!(spec.depends_on, ["hush"]);
        assert_eq!(spec.family, ModelFamily::SpeakerSuppression);
    }
}
