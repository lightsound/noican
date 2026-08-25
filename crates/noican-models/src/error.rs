//! Error type for model acquisition and inference.

use std::path::PathBuf;

/// Errors produced while locating, fetching, or running a model.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No model in the catalog has the requested identifier.
    #[error("unknown model id `{0}`")]
    UnknownModel(String),

    /// A weight file has not been downloaded yet.
    #[error("model file `{path}` is missing; run `noican fetch {model}` to download it")]
    MissingWeights {
        /// Identifier of the model whose file is missing.
        model: String,
        /// Where the file was expected.
        path: PathBuf,
    },

    /// A weight file exists but does not match its recorded digest.
    #[error(
        "checksum mismatch for `{path}`: expected sha256 {expected}, got {actual}. \
         Delete the file and fetch it again."
    )]
    ChecksumMismatch {
        /// The file that failed verification.
        path: PathBuf,
        /// Digest recorded in the catalog.
        expected: String,
        /// Digest computed from the file on disk.
        actual: String,
    },

    /// The download failed.
    #[error("failed to download {url}: {source}")]
    Download {
        /// The URL that could not be fetched.
        url: String,
        /// The underlying transport error.
        #[source]
        source: Box<ureq::Error>,
    },

    /// A filesystem operation failed.
    #[error("{operation} `{path}`: {source}")]
    Io {
        /// What was being attempted, for a readable message.
        operation: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// ONNX Runtime rejected the model or the inference call.
    #[error("onnx runtime error: {0}")]
    Runtime(#[from] ort::Error),

    /// The loaded graph does not have the signature its architecture requires.
    #[error("model `{model}` has an unexpected ONNX signature: {detail}")]
    UnexpectedSignature {
        /// Identifier of the offending model.
        model: String,
        /// What specifically did not line up.
        detail: String,
    },

    /// A required piece of ONNX metadata is absent or unparseable.
    #[error("model `{model}` is missing or has invalid metadata `{key}`")]
    Metadata {
        /// Identifier of the offending model.
        model: String,
        /// The metadata key at fault.
        key: String,
    },

    /// A core DSP primitive rejected its configuration or arguments.
    #[error(transparent)]
    Core(#[from] noican_core::Error),
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = core::result::Result<T, Error>;
