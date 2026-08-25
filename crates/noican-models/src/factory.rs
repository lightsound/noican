//! Creates ready-to-use [`Stage`]s from model identifiers.

use std::path::Path;

use noican_core::{Passthrough, Stage, StageError};

use crate::manifest::ModelSpec;

/// Identifier of the built-in bypass stage (always available, no weights).
pub const PASSTHROUGH_ID: &str = "passthrough";

/// Instantiates the stage for `id`, loading weights from `models_dir`
/// (fetch them first with [`crate::fetch::fetch_model`]).
///
/// # Errors
///
/// Returns [`StageError::Unsupported`] for unknown ids and
/// [`StageError::Inference`] when weights are missing or fail to load.
pub fn create_stage(id: &str, models_dir: &Path) -> Result<Box<dyn Stage>, StageError> {
    if id == PASSTHROUGH_ID {
        return Ok(Box::new(Passthrough));
    }
    let spec = ModelSpec::find(id)
        .ok_or_else(|| StageError::Unsupported(format!("unknown model id: {id}")))?;
    let _ = models_dir;
    Err(StageError::Unsupported(format!(
        "model {} is registered but its stage is not implemented yet",
        spec.id
    )))
}
