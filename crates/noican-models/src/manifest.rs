//! Static registry of supported models and their downloadable weights.
//!
//! Weights are never committed to the repository; they are fetched from the
//! official distribution points recorded in docs/tech-research.md §14 (see
//! [`crate::fetch`]).

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
    /// Files required at runtime.
    pub files: &'static [FileSpec],
    /// True when the model needs a speaker-enrollment embedding.
    pub needs_enrollment: bool,
}

impl ModelSpec {
    /// Looks a model up by [`ModelSpec::id`].
    #[must_use]
    pub fn find(id: &str) -> Option<&'static Self> {
        ALL_MODELS.iter().find(|m| m.id == id)
    }
}

/// All models known to this build. Populated by the stage modules.
pub static ALL_MODELS: &[ModelSpec] = &[];
