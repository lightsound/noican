//! Error type shared by the core DSP primitives.

/// Errors produced by the core audio primitives.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A caller passed a buffer whose length does not match the contract.
    #[error("buffer length mismatch: expected {expected} samples, got {actual}")]
    BufferLength {
        /// Length the callee requires.
        expected: usize,
        /// Length the caller supplied.
        actual: usize,
    },

    /// A stage was configured with parameters it cannot support.
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// A stage failed while processing a block.
    ///
    /// The inner message is produced off the audio thread (during setup) or by
    /// a non-real-time stage, so allocating here is acceptable.
    #[error("stage failed: {0}")]
    Stage(String),
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = core::result::Result<T, Error>;
