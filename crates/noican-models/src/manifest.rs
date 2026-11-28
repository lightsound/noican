//! See docs/adr/0009-model-catalog-fetch.md#model-weight-pinning.

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
    },
    ModelSpec {
        id: "dfn3",
        display_name: "DeepFilterNet3 48k",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "MIT OR Apache-2.0",
        files: &[],
        depends_on: &[],
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
    },
    ModelSpec {
        id: "hush-48k",
        display_name: "Hush 48k",
        family: ModelFamily::SpeakerSuppression,
        sample_rate: 48_000,
        license: "Apache-2.0",
        files: &[],
        depends_on: &["hush"],
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
