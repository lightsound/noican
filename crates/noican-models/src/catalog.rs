//! The catalog of models noican can run.
//!
//! Adding a model means adding one entry here plus, if its ONNX signature does
//! not match an existing [`Architecture`], one [`noican_core::Stage`]
//! implementation. Nothing in the engine, the CLI, or the UI changes.
//!
//! Every entry records a URL and a SHA-256 digest so that weights can be
//! fetched on demand and verified: model files are far too large to commit, and
//! silently corrupted weights produce plausible-sounding garbage rather than an
//! error.

use noican_core::WindowKind;

/// What a model is for.
///
/// The distinction matters to the UI, which groups the model picker, and to the
/// engine, which will eventually chain one of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// Removes non-speech noise.
    NoiseSuppression,
    /// Removes competing speakers as well as noise.
    SpeakerSuppression,
}

impl ModelKind {
    /// A short human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoiseSuppression => "noise suppression",
            Self::SpeakerSuppression => "speaker suppression",
        }
    }
}

/// Parameters of a spectral model whose ONNX graph does not carry them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpectralParams {
    /// Transform size.
    pub n_fft: usize,
    /// Hop between frames.
    pub hop: usize,
    /// Analysis (and synthesis) window the model was trained with.
    pub window: WindowKind,
}

/// How to drive a model's ONNX graph.
///
/// Each variant corresponds to one [`noican_core::Stage`] implementation. Every
/// one of them threads recurrent state across calls through the same
/// [`crate::session::CachedSession`]; they differ only in what the primary
/// inputs and outputs mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    /// Waveform in, waveform out; the graph contains its own transform.
    ///
    /// Used by `FastEnhancer`, whose ONNX export embeds a `DFT` node.
    Waveform,

    /// One spectrum frame in, one out. All transform parameters come from the
    /// graph's own metadata.
    ///
    /// Used by the `DPDFNet` family, which additionally seeds part of its state
    /// tensor from metadata (`erb_norm_init` and `spec_norm_init`).
    SpectralSelfDescribing,

    /// One spectrum frame in, one out, with the transform parameters supplied
    /// here because the graph does not carry them.
    ///
    /// Used by GTCRN and UL-UNAS, which share an I/O signature but were trained
    /// with different windows.
    Spectral(SpectralParams),
}

/// One file a model needs on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artifact {
    /// File name inside the model's directory.
    pub file_name: &'static str,
    /// Where to download it from.
    pub url: &'static str,
    /// Lowercase hex SHA-256 of the expected contents.
    pub sha256: &'static str,
}

/// Everything needed to describe, fetch, and instantiate one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDescriptor {
    /// Stable identifier used by the CLI, the config file, and the UI.
    pub id: &'static str,
    /// Name shown in the model picker.
    pub display_name: &'static str,
    /// What the model is for.
    pub kind: ModelKind,
    /// How to drive its graph.
    pub architecture: Architecture,
    /// Native sample rate in hertz.
    pub sample_rate: u32,
    /// Licence of the weights, for `THIRD_PARTY_NOTICES.md`.
    pub license: &'static str,
    /// Where the weights came from, for attribution and re-verification.
    pub source: &'static str,
    /// Anything a listener should know before judging the output.
    pub notes: &'static str,
    /// Files to download.
    pub artifacts: &'static [Artifact],
}

impl ModelDescriptor {
    /// The model's single ONNX file, for architectures that have exactly one.
    #[must_use]
    pub const fn primary_artifact(&self) -> &'static Artifact {
        &self.artifacts[0]
    }
}

macro_rules! fastenhancer {
    ($id:literal, $variant:literal, $display:literal, $sha:literal, $notes:literal) => {
        ModelDescriptor {
            id: $id,
            display_name: $display,
            kind: ModelKind::NoiseSuppression,
            architecture: Architecture::Waveform,
            sample_rate: 48_000,
            license: "MIT",
            source: "https://github.com/aask1357/fastenhancer (release onnx-48khz-v1)",
            notes: $notes,
            artifacts: &[Artifact {
                file_name: concat!("fastenhancer_", $variant, ".onnx"),
                url: concat!(
                    "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/\
                     fastenhancer_",
                    $variant,
                    ".onnx"
                ),
                sha256: $sha,
            }],
        }
    };
}

macro_rules! dpdfnet {
    ($id:literal, $file:literal, $display:literal, $rate:literal, $sha:literal, $notes:literal) => {
        ModelDescriptor {
            id: $id,
            display_name: $display,
            kind: ModelKind::NoiseSuppression,
            architecture: Architecture::SpectralSelfDescribing,
            sample_rate: $rate,
            license: "Apache-2.0",
            source: "https://github.com/k2-fsa/sherpa-onnx \
                     (release speech-enhancement-models; models by Ceva Inc.)",
            notes: $notes,
            artifacts: &[Artifact {
                file_name: concat!($file, ".onnx"),
                url: concat!(
                    "https://github.com/k2-fsa/sherpa-onnx/releases/download/\
                     speech-enhancement-models/",
                    $file,
                    ".onnx"
                ),
                sha256: $sha,
            }],
        }
    };
}

/// Every model noican knows how to run, in the order the UI shows them.
///
/// Digests were computed from the files actually downloaded on 2026-08-25; see
/// `docs/tech-research.md` §5.4 for the two artefacts the research named that
/// turned out not to be published.
pub static CATALOG: &[ModelDescriptor] = &[
    fastenhancer!(
        "fastenhancer-t",
        "t",
        "FastEnhancer T (48 kHz)",
        "1993a3f58ae95d959123b3d28779c2800e0f016d6f4e1177f1213144f301b89c",
        "Smallest of the five variants at 28K parameters. The primary candidate for the live path."
    ),
    fastenhancer!(
        "fastenhancer-s",
        "s",
        "FastEnhancer S (48 kHz)",
        "f04ece2beed330da367264c54cedded62f65a117fbde5c005d3a88fc796d0ba3",
        "One step up from T; the recommended starting point for quality comparisons."
    ),
    fastenhancer!(
        "fastenhancer-b",
        "b",
        "FastEnhancer B (48 kHz)",
        "70e23bba3d41e80d30ebc5eba39d9df64f0e0315f31c772022bb17576c4d96bf",
        "Base variant, 207K parameters."
    ),
    fastenhancer!(
        "fastenhancer-m",
        "m",
        "FastEnhancer M (48 kHz)",
        "c7da800810b583f4734d757c6e14d235f3eec81476121b595743e5866b66efa2",
        "Medium variant; heavier than the published latency measurements cover."
    ),
    fastenhancer!(
        "fastenhancer-l",
        "l",
        "FastEnhancer L (48 kHz)",
        "d7138309ec98266c668b2f86658fcc8f4e82ded67cd7fc8d6598534104b4bf89",
        "Largest variant; included for the quality ceiling, not for the live path."
    ),
    dpdfnet!(
        "dpdfnet2-48k-hr",
        "dpdfnet2_48khz_hr",
        "DPDFNet-2 48 kHz HR",
        48_000,
        "0b399f8a58dc4d70d8cd97541f5c39869406145193b957d00a03b66070944928",
        "The only 48 kHz DPDFNet published. Claims some dereverberation as well as denoising."
    ),
    dpdfnet!(
        "dpdfnet2-16k",
        "dpdfnet2",
        "DPDFNet-2 (16 kHz)",
        16_000,
        "ce35d6025fc71df0ef10d1540e1b7916837bbfe5f6896deb744508d2cad487a9",
        "16 kHz sibling of the 48 kHz HR model; useful for isolating how much the rate matters."
    ),
    dpdfnet!(
        "dpdfnet4-16k",
        "dpdfnet4",
        "DPDFNet-4 (16 kHz)",
        16_000,
        "71b588bc26163941aa82a592cce924b08c1fbdc0879fe44f5a2d4eac44bd8420",
        "Middle of the 16 kHz range."
    ),
    dpdfnet!(
        "dpdfnet8-16k",
        "dpdfnet8",
        "DPDFNet-8 (16 kHz)",
        16_000,
        "2751c1f5a4e849d23a07c675b4c838158b249b42152f10cc318522dd339134f0",
        "Heaviest DPDFNet variant published. Stands in for dpdfnet8_48khz_hr, which does not exist."
    ),
    ModelDescriptor {
        id: "gtcrn",
        display_name: "GTCRN (16 kHz)",
        kind: ModelKind::NoiseSuppression,
        architecture: Architecture::Spectral(SpectralParams {
            n_fft: 512,
            hop: 256,
            window: WindowKind::HannSqrt,
        }),
        sample_rate: 16_000,
        license: "Apache-2.0",
        source: "https://github.com/Xiaobin-Rong/gtcrn \
                 (redistributed via sherpa-onnx speech-enhancement-models)",
        notes: "48K parameters. Superseded by UL-UNAS but the simplest graph in the catalog, \
                which makes it a useful sanity reference.",
        artifacts: &[Artifact {
            file_name: "gtcrn_simple.onnx",
            url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/\
                  speech-enhancement-models/gtcrn_simple.onnx",
            sha256: "e77603ac0c23dac3227dd2d7135b3a585cbee2679048aecfa886657d3ae1b534",
        }],
    },
    ModelDescriptor {
        id: "ul-unas",
        display_name: "UL-UNAS (16 kHz)",
        kind: ModelKind::NoiseSuppression,
        architecture: Architecture::Spectral(SpectralParams {
            n_fft: 512,
            hop: 256,
            window: WindowKind::Hann,
        }),
        sample_rate: 16_000,
        license: "Apache-2.0",
        source: "https://github.com/Xiaobin-Rong/ul-unas",
        notes: "GTCRN's successor at ~171K parameters, and the low-latency-mode candidate. \
                Same graph signature as GTCRN but trained with a plain Hann window.",
        artifacts: &[Artifact {
            file_name: "ulunas_stream_simple.onnx",
            // Pinned to a commit: the file lives in the repository tree rather
            // than a release, so a branch URL would not be reproducible.
            url: "https://raw.githubusercontent.com/Xiaobin-Rong/ul-unas/\
                  a2d77c933ea17bc17dbc9ad5c942c857d0d8e9ee/ulunas_onnx/onnx_models/\
                  ulunas_stream_simple.onnx",
            sha256: "f2e804d54d6a88f4f82f44d86c9f1cf646db2509bfca935cfbfc5fcd8cbfac3b",
        }],
    },
];

/// Looks up a model by identifier.
#[must_use]
pub fn find(id: &str) -> Option<&'static ModelDescriptor> {
    CATALOG.iter().find(|model| model.id == id)
}

/// All catalog identifiers, in display order.
pub fn ids() -> impl Iterator<Item = &'static str> {
    CATALOG.iter().map(|model| model.id)
}

#[cfg(test)]
mod tests {
    use super::{CATALOG, ModelKind, find, ids};

    #[test]
    fn ids_are_unique() {
        let mut seen = Vec::new();
        for id in ids() {
            assert!(!seen.contains(&id), "duplicate model id `{id}`");
            seen.push(id);
        }
        assert_eq!(seen.len(), CATALOG.len());
    }

    #[test]
    fn every_entry_is_self_consistent() {
        for model in CATALOG {
            assert!(!model.artifacts.is_empty(), "{} has no artifacts", model.id);
            assert!(
                !model.notes.is_empty() && !model.license.is_empty() && !model.source.is_empty(),
                "{} is missing attribution",
                model.id
            );
            assert!(
                matches!(model.sample_rate, 16_000 | 48_000),
                "{} has an unexpected rate {}",
                model.id,
                model.sample_rate
            );
            for artifact in model.artifacts {
                assert_eq!(
                    artifact.sha256.len(),
                    64,
                    "{} has a malformed digest",
                    model.id
                );
                assert!(
                    artifact.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                    "{} has a non-hex digest",
                    model.id
                );
                assert!(
                    artifact.url.starts_with("https://"),
                    "{} has a non-HTTPS url",
                    model.id
                );
                assert!(
                    !artifact.url.contains(' '),
                    "{} has whitespace in its url, which means a string-continuation escape was \
                     missed: {}",
                    model.id,
                    artifact.url
                );
                assert_eq!(
                    std::path::Path::new(artifact.file_name).extension(),
                    Some(std::ffi::OsStr::new("onnx")),
                    "{} has a non-ONNX artifact",
                    model.id
                );
            }
        }
    }

    #[test]
    fn lookup_finds_catalog_entries() {
        assert_eq!(find("fastenhancer-t").map(|m| m.id), Some("fastenhancer-t"));
        assert!(find("no-such-model").is_none());
        assert_eq!(
            find("dpdfnet2-48k-hr").map(|m| m.kind),
            Some(ModelKind::NoiseSuppression)
        );
        assert_eq!(ModelKind::SpeakerSuppression.label(), "speaker suppression");
    }
}
