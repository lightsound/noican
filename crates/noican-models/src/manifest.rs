//! Static registry of supported models and their downloadable weights.
//!
//! Weights are never committed to the repository; they are fetched from the
//! official distribution points recorded in docs/tech-research.md §14 (see
//! [`crate::fetch`] and docs/models.md). SHA-256 digests are pinned to the
//! artifacts verified during Phase 0 bring-up.

/// Broad role of a model in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Suppresses non-speech noise.
    Denoise,
    /// Suppresses background speakers (keeps the target speaker).
    SpeakerSuppression,
    /// Produces speaker embeddings (support model, not a pipeline stage).
    SpeakerEmbedding,
}

/// One downloadable file belonging to a model.
#[derive(Debug, Clone, Copy)]
pub struct FileSpec {
    /// File name under the model's directory.
    pub name: &'static str,
    /// Direct download URL (official release asset).
    pub url: &'static str,
    /// Expected SHA-256 (lowercase hex), when pinned.
    pub sha256: Option<&'static str>,
}

/// A model available to the engine and CLI.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    /// Stable identifier (CLI argument, config value, UI key).
    pub id: &'static str,
    /// Human-readable name for UIs.
    pub display_name: &'static str,
    /// Pipeline role.
    pub family: ModelFamily,
    /// Native processing rate (Hz).
    pub sample_rate: u32,
    /// License of the weights (informational; see `THIRD_PARTY_NOTICES.md`).
    pub license: &'static str,
    /// Files required at runtime (empty when the model is embedded in the
    /// binary).
    pub files: &'static [FileSpec],
    /// True when the model needs a speaker-enrollment embedding.
    pub needs_enrollment: bool,
    /// Set when the distribution point currently requires authentication
    /// or is otherwise not freely fetchable; the note explains the state.
    pub fetch_note: Option<&'static str>,
}

impl ModelSpec {
    /// Looks a model up by [`ModelSpec::id`].
    #[must_use]
    pub fn find(id: &str) -> Option<&'static Self> {
        ALL_MODELS.iter().find(|m| m.id == id)
    }

    /// Models that can be selected as processing stages (excludes support
    /// models such as speaker-embedding extractors).
    pub fn stages() -> impl Iterator<Item = &'static Self> {
        ALL_MODELS
            .iter()
            .filter(|m| m.family != ModelFamily::SpeakerEmbedding)
    }
}

macro_rules! fastenhancer {
    ($id:literal, $name:literal, $file:literal, $url:literal, $sha:literal) => {
        ModelSpec {
            id: $id,
            display_name: $name,
            family: ModelFamily::Denoise,
            sample_rate: 48_000,
            license: "MIT",
            files: &[FileSpec {
                name: $file,
                url: $url,
                sha256: Some($sha),
            }],
            needs_enrollment: false,
            fetch_note: None,
        }
    };
}

/// All models known to this build.
pub static ALL_MODELS: &[ModelSpec] = &[
    fastenhancer!(
        "fastenhancer-t",
        "FastEnhancer-T 48k",
        "fastenhancer_t.onnx",
        "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_t.onnx",
        "1993a3f58ae95d959123b3d28779c2800e0f016d6f4e1177f1213144f301b89c"
    ),
    fastenhancer!(
        "fastenhancer-b",
        "FastEnhancer-B 48k",
        "fastenhancer_b.onnx",
        "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_b.onnx",
        "70e23bba3d41e80d30ebc5eba39d9df64f0e0315f31c772022bb17576c4d96bf"
    ),
    fastenhancer!(
        "fastenhancer-s",
        "FastEnhancer-S 48k",
        "fastenhancer_s.onnx",
        "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_s.onnx",
        "f04ece2beed330da367264c54cedded62f65a117fbde5c005d3a88fc796d0ba3"
    ),
    fastenhancer!(
        "fastenhancer-m",
        "FastEnhancer-M 48k",
        "fastenhancer_m.onnx",
        "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_m.onnx",
        "c7da800810b583f4734d757c6e14d235f3eec81476121b595743e5866b66efa2"
    ),
    fastenhancer!(
        "fastenhancer-l",
        "FastEnhancer-L 48k",
        "fastenhancer_l.onnx",
        "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_l.onnx",
        "d7138309ec98266c668b2f86658fcc8f4e82ded67cd7fc8d6598534104b4bf89"
    ),
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
        needs_enrollment: false,
        fetch_note: None,
    },
    ModelSpec {
        id: "dpdfnet8",
        display_name: "DPDFNet8 48k HR",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "Apache-2.0",
        files: &[FileSpec {
            // Not on the sherpa-onnx GitHub release yet (docs are ahead of
            // the release); the official ceva-ip Hugging Face repo hosts it.
            name: "dpdfnet8_48khz_hr.onnx",
            url: "https://huggingface.co/Ceva-IP/DPDFNet/resolve/main/onnx/dpdfnet8_48khz_hr.onnx",
            sha256: Some("7b3afbb260a08fe9af3d16e3bda992971be1e7e951d1dee7c2d235f5c43f5631"),
        }],
        needs_enrollment: false,
        fetch_note: None,
    },
    ModelSpec {
        id: "dfn3",
        display_name: "DeepFilterNet3 48k",
        family: ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "MIT OR Apache-2.0",
        // Embedded in the deep_filter crate (default-model feature).
        files: &[],
        needs_enrollment: false,
        fetch_note: None,
    },
    ModelSpec {
        id: "ul-unas",
        display_name: "UL-UNAS 16k",
        family: ModelFamily::Denoise,
        sample_rate: 16_000,
        license: "MIT",
        files: &[FileSpec {
            // Commit-pinned permalink (the repo has no releases).
            name: "ulunas_stream_simple.onnx",
            url: "https://raw.githubusercontent.com/Xiaobin-Rong/ul-unas/00f7c700da43d38347f30a6ccebd86fcbc798e07/ulunas_onnx/onnx_models/ulunas_stream_simple.onnx",
            sha256: Some("f2e804d54d6a88f4f82f44d86c9f1cf646db2509bfca935cfbfc5fcd8cbfac3b"),
        }],
        needs_enrollment: false,
        fetch_note: None,
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
        needs_enrollment: false,
        fetch_note: None,
    },
    ModelSpec {
        id: "tse-48k",
        display_name: "TSE Conv-TasNet 48k",
        family: ModelFamily::SpeakerSuppression,
        sample_rate: 48_000,
        license: "unknown (repo currently private)",
        files: &[
            FileSpec {
                name: "tse_prod_48k.onnx",
                url: "https://huggingface.co/penta2himajin/tse-conv-tasnet-48k/resolve/main/tse_prod_48k.onnx",
                sha256: None,
            },
            FileSpec {
                name: "tse_prod_48k.onnx.data",
                url: "https://huggingface.co/penta2himajin/tse-conv-tasnet-48k/resolve/main/tse_prod_48k.onnx.data",
                sha256: None,
            },
        ],
        needs_enrollment: true,
        fetch_note: Some(
            "the Hugging Face repo penta2himajin/tse-conv-tasnet-48k currently returns \
             HTTP 401 (private); set HF_TOKEN if you have access, or place the files \
             manually (see docs/models.md)",
        ),
    },
    ModelSpec {
        id: "ecapa-tdnn",
        display_name: "ECAPA-TDNN embedding",
        family: ModelFamily::SpeakerEmbedding,
        sample_rate: 16_000,
        license: "Apache-2.0",
        files: &[FileSpec {
            name: "ecapa_tdnn.onnx",
            url: "https://huggingface.co/penta2himajin/ecapa-tdnn-onnx/resolve/main/ecapa_tdnn.onnx",
            sha256: Some("75f5f36d23879c5b2dd73b09221e8727e8e6e6a7cbd1a0655992d7ae81195698"),
        }],
        needs_enrollment: false,
        fetch_note: None,
    },
];
