//! Shared ONNX Runtime helpers for streaming model stages.

use std::path::Path;

use noican_core::StageError;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

/// Loads an ONNX session tuned for low-latency streaming inference on CPU:
/// single-threaded (frames are small; thread wakeups cost more than they
/// save) with full graph optimization.
///
/// # Errors
///
/// Returns [`StageError::Inference`] when the file cannot be loaded.
pub fn load_streaming_session(path: &Path) -> Result<Session, StageError> {
    let build = || -> ort::Result<Session> {
        Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_file(path)
    };
    build().map_err(|e| StageError::Inference(format!("failed to load {}: {e}", path.display())))
}

/// Maps an [`ort::Error`] into a [`StageError`] with context.
#[must_use]
pub fn inference_error(context: &str, e: &ort::Error) -> StageError {
    StageError::Inference(format!("{context}: {e}"))
}
