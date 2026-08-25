//! AUHAL transport between a private Aggregate Device and the Rust engine.

use noican_engine::SwitchingEngine;
use thiserror::Error;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::Runtime;

/// AUHAL lifecycle failures.
#[derive(Debug, Error)]
pub enum CoreAudioError {
    /// A Core Audio call returned a nonzero `OSStatus`.
    #[error("{operation} failed with OSStatus {status}")]
    Status {
        /// Operation being performed.
        operation: &'static str,
        /// Core Audio status code.
        status: i32,
    },
    /// No AUHAL component is installed.
    #[error("kAudioUnitSubType_HALOutput is unavailable")]
    MissingAuHal,
    /// The processing worker could not start.
    #[error("audio processing worker failed: {0}")]
    Worker(String),
    /// This build does not target macOS.
    #[error("AUHAL is available only on macOS")]
    UnsupportedPlatform,
}

#[cfg(not(target_os = "macos"))]
/// Non-macOS placeholder that keeps workspace checks portable.
pub struct Runtime;

#[cfg(not(target_os = "macos"))]
impl Runtime {
    /// Reject startup on non-macOS targets.
    ///
    /// # Errors
    ///
    /// Always returns [`CoreAudioError::UnsupportedPlatform`].
    pub fn start(_aggregate_device: u32, _engine: SwitchingEngine) -> Result<Self, CoreAudioError> {
        Err(CoreAudioError::UnsupportedPlatform)
    }

    /// No-op portable shutdown.
    pub const fn stop(&mut self) {}

    /// Portable builds never run an audio device.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        false
    }

    /// Portable builds cannot report a callback fault.
    #[must_use]
    pub const fn is_faulted(&self) -> bool {
        false
    }
}
