//! Error types shared by all stages.

/// Error produced by an audio-processing stage.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    /// The model/inference backend failed.
    #[error("inference failed: {0}")]
    Inference(String),
    /// The caller passed buffers with the wrong length.
    #[error("buffer length mismatch: expected {expected}, got {got}")]
    BufferLen {
        /// Expected number of samples.
        expected: usize,
        /// Actual number of samples.
        got: usize,
    },
    /// The stage cannot operate at the requested rate/geometry.
    #[error("unsupported configuration: {0}")]
    Unsupported(String),
}
