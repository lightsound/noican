//! Creates ready-to-use [`Stage`]s from model identifiers.

use std::path::Path;

use noican_core::{Passthrough, Stage, StageError};

use crate::manifest::{ALL_MODELS, ModelSpec};
use crate::traits::ModelTraits;

/// Identifier of the built-in bypass stage (always available, no weights).
pub const PASSTHROUGH_ID: &str = "passthrough";

/// See docs/adr/0009-model-catalog-fetch.md#model-catalog-single-source.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    /// See docs/adr/0009-model-catalog-fetch.md#model-catalog-single-source.
    pub id: &'static str,
    /// Human-readable name for UIs.
    pub display_name: &'static str,
    /// Picker-facing characteristics (ratings, tagline, details).
    pub traits: ModelTraits,
}

/// The selectable catalog: the bypass followed by every registry model.
pub fn catalog() -> impl Iterator<Item = CatalogEntry> {
    std::iter::once(CatalogEntry {
        id: PASSTHROUGH_ID,
        display_name: "Passthrough (no processing)",
        traits: ModelTraits::PASSTHROUGH,
    })
    .chain(ALL_MODELS.iter().map(|spec| CatalogEntry {
        id: spec.id,
        display_name: spec.display_name,
        traits: spec.traits,
    }))
}

/// See docs/adr/0001-realtime-audio-rules.md#buffer-presizing-contract.
pub const MAX_BLOCK_LEN: usize = 2048;

/// See docs/adr/0009-model-catalog-fetch.md#model-weight-digest-verification.
///
/// # Errors
///
/// Returns [`StageError::Unsupported`] for unknown ids and
/// [`StageError::Inference`] when weights are missing, fail
/// verification, or fail to load.
pub fn create_stage(id: &str, models_dir: &Path) -> Result<Box<dyn Stage>, StageError> {
    if id == PASSTHROUGH_ID {
        return Ok(Box::new(Passthrough));
    }
    let spec = ModelSpec::find(id)
        .ok_or_else(|| StageError::Unsupported(format!("unknown model id: {id}")))?;
    crate::fetch::verify_model(models_dir, spec).map_err(|error| {
        StageError::Inference(format!("verifying weights for {}: {error}", spec.id))
    })?;
    (spec.build)(spec, models_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_with_weights_refuses_to_build_without_them() {
        let empty_dir = std::env::temp_dir().join("noican-factory-test-no-weights");
        for spec in ALL_MODELS {
            if spec.files.is_empty() && spec.depends_on.is_empty() {
                continue;
            }
            match create_stage(spec.id, &empty_dir) {
                Err(StageError::Inference(_)) => {}
                Err(other) => panic!("{}: unexpected error {other}", spec.id),
                Ok(_) => panic!("{}: loaded without weights", spec.id),
            }
        }
    }
}
