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

    /// A speaker-embedding graph driving an enrolment gate.
    ///
    /// Unlike every other entry, this one needs state that is not in the
    /// catalog: an enrolled profile, which the user creates with `noican
    /// enroll`. Without one there is nobody to recognise.
    SpeakerGate,

    /// A three-graph `DeepFilterNet` bundle: an encoder, an ERB-mask decoder,
    /// and a deep-filter decoder, plus a `config.ini` giving every parameter.
    ///
    /// Everything outside those graphs is ours: the transform, the ERB and
    /// complex features, the mask interpolation, and the deep filter itself.
    DeepFilterNet,
}

/// What a downloaded file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A single ONNX graph, used as downloaded.
    Graph,
    /// A gzipped tar holding several graphs and a `config.ini`, which has to be
    /// unpacked before use.
    ///
    /// Paths inside are flattened to their file names: the two published bundles
    /// disagree about whether the files sit at the root or under a directory,
    /// and nothing downstream cares.
    Bundle,
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
    /// Whether the file is used as-is or unpacked.
    pub kind: ArtifactKind,
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
    /// Whether the model is a candidate for the live path.
    ///
    /// False for models whose published graphs force block processing, which
    /// costs seconds of latency (§5.5). They stay selectable because the offline
    /// comparison is what they are for, but anything offering the choice has to
    /// say so — a user who picks one unwarned gets several seconds of silence
    /// and concludes the app is broken.
    pub live_capable: bool,
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
            live_capable: true,
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
                kind: ArtifactKind::Graph,
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
            live_capable: true,
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
                kind: ArtifactKind::Graph,
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
        live_capable: true,
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
            kind: ArtifactKind::Graph,
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
        live_capable: true,
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
            kind: ArtifactKind::Graph,
        }],
    },
    ModelDescriptor {
        id: "deepfilternet3",
        display_name: "DeepFilterNet3 (48 kHz)",
        kind: ModelKind::NoiseSuppression,
        architecture: Architecture::DeepFilterNet,
        sample_rate: 48_000,
        live_capable: false,
        license: "MIT OR Apache-2.0",
        source: "https://github.com/Rikorose/DeepFilterNet (models/DeepFilterNet3_onnx.tar.gz)",
        notes: "The historical baseline and the reference row in comparisons. Runs as a block \
                stage: its published graphs take no recurrent state, so they cannot be driven \
                frame by frame (see docs/tech-research.md §5.5).",
        artifacts: &[Artifact {
            file_name: "DeepFilterNet3_onnx.tar.gz",
            // Pinned to a commit: the bundle lives in the repository tree rather
            // than a release, so a branch URL would not be reproducible.
            url: "https://raw.githubusercontent.com/Rikorose/DeepFilterNet/\
                  d375b2d8309e0935d165700c91da9de862a99c31/models/DeepFilterNet3_onnx.tar.gz",
            sha256: "c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616",
            kind: ArtifactKind::Bundle,
        }],
    },
    ModelDescriptor {
        id: "hush",
        display_name: "Hush (16 kHz, background speakers)",
        kind: ModelKind::SpeakerSuppression,
        architecture: Architecture::DeepFilterNet,
        sample_rate: 16_000,
        live_capable: false,
        license: "Apache-2.0",
        source: "https://huggingface.co/weya-ai/hush",
        notes: "DeepFilterNet3 retrained with competing speakers, and the only model here that \
                targets them. Needs no enrollment, but suppresses the quieter speaker, so it can \
                fail when the interferer is louder. Attenuates heavily — around 14 dB even on \
                clean speech, which is the model's own behaviour and was confirmed against its \
                reference runtime. Also a block stage, for the same reason as DeepFilterNet3.",
        artifacts: &[Artifact {
            file_name: "hush_onnx.tar.gz",
            // Pinned to a revision: `main` moves.
            url: "https://huggingface.co/weya-ai/hush/resolve/\
                  a55d932cbf6344d284ac985f21e7f6e5bc4d38a5/onnx/\
                  advanced_dfnet16k_model_best_onnx.tar.gz",
            sha256: "45632ccaa82b71bb743d6caa7c78e983fe2f2790a3af7f6ec48e6ed7ba085df6",
            kind: ArtifactKind::Bundle,
        }],
    },
    ModelDescriptor {
        id: "speaker-gate",
        display_name: "Speaker gate (ECAPA-TDNN enrolment)",
        kind: ModelKind::SpeakerSuppression,
        architecture: Architecture::SpeakerGate,
        sample_rate: 16_000,
        live_capable: true,
        license: "Apache-2.0",
        source: "https://huggingface.co/penta2himajin/ecapa-tdnn-onnx (SpeechBrain \
                 spkrec-ecapa-voxceleb, 192-dim)",
        notes: "Attenuates audio when the dominant speaker is not the enrolled one. Needs \
                `noican enroll` first, and needs about 1.5 s of speech to decide, so it \
                suppresses a sustained other voice rather than a single interjected word. \
                Complementary to Hush, which separates overlapping speakers within a frame but \
                cannot be told who you are.",
        artifacts: &[Artifact {
            file_name: "ecapa_tdnn.onnx",
            // Pinned to a revision: `main` moves.
            url: "https://huggingface.co/penta2himajin/ecapa-tdnn-onnx/resolve/\
                  57bc773c7cc1a8afa117b38b0b2a38c96ffa99a2/ecapa_tdnn.onnx",
            sha256: "75f5f36d23879c5b2dd73b09221e8727e8e6e6a7cbd1a0655992d7ae81195698",
            kind: ArtifactKind::Graph,
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
    use super::{Architecture, ArtifactKind, CATALOG, ModelKind, find, ids};

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
                let name = std::path::Path::new(artifact.file_name);
                let extension = name.extension().and_then(std::ffi::OsStr::to_str);
                match artifact.kind {
                    ArtifactKind::Graph => assert_eq!(
                        extension,
                        Some("onnx"),
                        "{} declares a graph artifact that is not ONNX: {:?}",
                        model.id,
                        name
                    ),
                    ArtifactKind::Bundle => assert_eq!(
                        extension,
                        Some("gz"),
                        "{} declares a bundle artifact that is not gzipped: {:?}",
                        model.id,
                        name
                    ),
                }
            }
        }
    }

    /// `THIRD_PARTY_NOTICES.md` has to cover every licence and every upstream
    /// the catalog draws on. The realistic way that breaks is somebody adding a
    /// model from a new source and forgetting the notice, so this fails the
    /// build rather than leaving it to review.
    #[test]
    fn every_licence_and_source_appears_in_the_notices() {
        let notices = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../THIRD_PARTY_NOTICES.md"
        ));

        for model in CATALOG {
            assert!(
                notices.contains(model.license),
                "{} is licensed {} and THIRD_PARTY_NOTICES.md does not mention that licence",
                model.id,
                model.license
            );

            // The repository or dataset the weights come from, without the
            // scheme or the parenthetical the catalog sometimes appends.
            let upstream = model
                .source
                .split_whitespace()
                .next()
                .expect("a source is never blank")
                .trim_start_matches("https://");
            assert!(
                notices.contains(upstream),
                "{} comes from {upstream} and THIRD_PARTY_NOTICES.md does not mention it",
                model.id
            );
        }
    }

    /// The flag exists so a picker can warn before a user chooses a model that
    /// will hand them seconds of silence, so it has to agree with which stage
    /// implementation the model actually gets.
    #[test]
    fn only_block_stage_models_are_marked_unfit_for_live_use() {
        for model in CATALOG {
            let forced_into_blocks = matches!(model.architecture, Architecture::DeepFilterNet);
            assert_eq!(
                model.live_capable, !forced_into_blocks,
                "{} claims live_capable = {} but its architecture disagrees",
                model.id, model.live_capable
            );
        }
        assert!(
            CATALOG.iter().any(|model| !model.live_capable),
            "nothing is marked unfit for live use, so the flag is untested"
        );
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
