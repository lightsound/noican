//! Creates ready-to-use [`Stage`]s from model identifiers.

use std::path::Path;

use noican_core::{FramedStage, Passthrough, Stage, StageError};

use crate::fetch::model_dir;
use crate::manifest::ModelSpec;
use crate::stages::dfn_tract::DfTractStage;
use crate::stages::dpdfnet::DpdfnetStage;
use crate::stages::fastenhancer::FastEnhancerStage;
use crate::stages::tse::TseStage;
use crate::stages::ulunas::UlunasStage;

/// Identifier of the built-in bypass stage (always available, no weights).
pub const PASSTHROUGH_ID: &str = "passthrough";

/// Largest engine block the returned stages are pre-sized for (larger
/// blocks still work at the cost of a reallocation).
pub const MAX_BLOCK_LEN: usize = 2048;

/// Options for stage construction.
#[derive(Debug, Default, Clone)]
pub struct StageOptions {
    /// 192-dim speaker-enrollment embedding, required by models with
    /// [`ModelSpec::needs_enrollment`] (compute it with
    /// [`crate::embedding::EcapaEmbedder`]).
    pub enrollment: Option<Vec<f32>>,
}

fn file_path(models_dir: &Path, spec: &ModelSpec, index: usize) -> std::path::PathBuf {
    model_dir(models_dir, spec).join(spec.files[index].name)
}

/// Instantiates the stage for `id`, loading weights from `models_dir`
/// (fetch them first with [`crate::fetch::fetch_model`]).
///
/// # Errors
///
/// Returns [`StageError::Unsupported`] for unknown or non-stage ids and
/// [`StageError::Inference`] when weights are missing or fail to load.
pub fn create_stage(
    id: &str,
    models_dir: &Path,
    options: &StageOptions,
) -> Result<Box<dyn Stage>, StageError> {
    if id == PASSTHROUGH_ID {
        return Ok(Box::new(Passthrough));
    }
    let spec = ModelSpec::find(id)
        .ok_or_else(|| StageError::Unsupported(format!("unknown model id: {id}")))?;
    match spec.id {
        "fastenhancer-t" | "fastenhancer-b" | "fastenhancer-s" | "fastenhancer-m"
        | "fastenhancer-l" => {
            let stage = FastEnhancerStage::new(spec.id, &file_path(models_dir, spec, 0))?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "dpdfnet2" | "dpdfnet8" => {
            let stage = DpdfnetStage::new(spec.id, &file_path(models_dir, spec, 0))?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        "dfn3" => {
            let stage = DfTractStage::deepfilternet3(spec.id)?;
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
        "tse-48k" => {
            let embedding = options.enrollment.as_deref().ok_or_else(|| {
                StageError::Unsupported(
                    "tse-48k needs a speaker enrollment (pass an enrollment clip; \
                     CLI: --enroll <wav>)"
                        .to_owned(),
                )
            })?;
            let stage = TseStage::new(spec.id, &file_path(models_dir, spec, 0), embedding)?;
            Ok(Box::new(FramedStage::new(stage, MAX_BLOCK_LEN)?))
        }
        other => Err(StageError::Unsupported(format!(
            "{other} is not a processing stage"
        ))),
    }
}
