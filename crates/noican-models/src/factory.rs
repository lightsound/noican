//! Creates ready-to-use [`Stage`]s from model identifiers.

use std::path::Path;

use noican_core::{FramedStage, Passthrough, Stage, StageError};

use crate::fetch::model_dir;
use crate::manifest::{ALL_MODELS, ModelSpec};
use crate::stages::dfn_tract::DfTractStage;
use crate::stages::dpdfnet::DpdfnetStage;
use crate::stages::hush_wideband::HushWidebandStage;
use crate::stages::ulunas::UlunasStage;

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
    pub traits: crate::traits::ModelTraits,
}

/// The selectable catalog: the bypass followed by every registry model.
pub fn catalog() -> impl Iterator<Item = CatalogEntry> {
    std::iter::once(CatalogEntry {
        id: PASSTHROUGH_ID,
        display_name: "Passthrough (no processing)",
        traits: crate::traits::ModelTraits::for_id(PASSTHROUGH_ID),
    })
    .chain(ALL_MODELS.iter().map(|spec| CatalogEntry {
        id: spec.id,
        display_name: spec.display_name,
        traits: crate::traits::ModelTraits::for_id(spec.id),
    }))
}

/// See docs/adr/0001-realtime-audio-rules.md#buffer-presizing-contract.
pub const MAX_BLOCK_LEN: usize = 2048;

fn file_path(models_dir: &Path, spec: &ModelSpec, index: usize) -> std::path::PathBuf {
    model_dir(models_dir, spec).join(spec.files[index].name)
}

fn dependency_file_path(
    models_dir: &Path,
    spec: &ModelSpec,
    dependency: &str,
    index: usize,
) -> Result<std::path::PathBuf, StageError> {
    let dep = spec
        .dependencies()
        .find(|dep| dep.id == dependency)
        .ok_or_else(|| {
            StageError::Unsupported(format!(
                "{} does not depend on {dependency} (registry inconsistency)",
                spec.id
            ))
        })?;
    Ok(file_path(models_dir, dep, index))
}

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
    match spec.id {
        "dpdfnet2" | "dpdfnet8" => {
            let stage = DpdfnetStage::new(spec.id, &file_path(models_dir, spec, 0))?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "dfn3" => {
            let stage = DfTractStage::deepfilternet3(spec.id)?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "dfn3-ll" => {
            let stage = DfTractStage::deepfilternet3_ll(spec.id, &file_path(models_dir, spec, 0))?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "ul-unas" => {
            let stage = UlunasStage::new(spec.id, &file_path(models_dir, spec, 0))?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "hush" => {
            let stage = DfTractStage::hush(spec.id, &file_path(models_dir, spec, 0))?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "hush-48k" => {
            let tarball = dependency_file_path(models_dir, spec, "hush", 0)?;
            let stage = HushWidebandStage::new(spec.id, &tarball)?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        other => Err(StageError::Unsupported(format!(
            "{other} has no stage implementation (registry inconsistency)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registry_entry_has_a_stage_implementation() {
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
