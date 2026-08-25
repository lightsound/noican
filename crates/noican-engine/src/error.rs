//! Error type for the engine.

/// Errors produced while configuring or driving the engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A stage could not be adapted to the host format.
    #[error(transparent)]
    Core(#[from] noican_core::Error),

    /// The inference thread has not picked up the previous stage yet.
    ///
    /// Only possible if switches are requested faster than the ramp completes.
    #[error("a stage switch is already in flight; try again once the ramp completes")]
    SwitchInFlight,

    /// The inference thread has stopped.
    #[error("the engine is not running")]
    NotRunning,

    /// The engine is already running.
    #[error("the engine is already running")]
    AlreadyRunning,

    /// A configuration value is out of range.
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
}

/// Convenience alias for results produced by this crate.
pub type Result<T> = core::result::Result<T, Error>;
