//! AUHAL transport between a private Aggregate Device and the Rust engine.
//!
//! Ported from the Phase 0 transport candidate that passed hardware
//! acceptance on macOS 26 / Apple Silicon (mic permission → Running,
//! QuickTime recording through `BlackHole` 2ch, clean teardown), rewired to
//! drive [`noican_core::SwitchingEngine`] so the processing path — including
//! the 16 kHz polyphase resampling in [`noican_core::FramedStage`] — is the
//! exact code the CLI comparison mode exercises.
//!
//! Real-time rules (docs/tech-research.md §9): the Core Audio render
//! callback only calls `AudioUnitRender`, moves `f32` samples through
//! preallocated lock-free SPSC rings, and signals a dispatch semaphore
//! (non-blocking); inference runs on a dedicated worker thread joined to
//! the device's `os_workgroup`, blocking on that semaphore between
//! callbacks instead of spinning; output-ring underrun produces silence,
//! never blocking the callback.

use thiserror::Error;

#[cfg(not(target_os = "macos"))]
use noican_core::SwitchingEngine;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::Runtime;

/// Samples per engine block driven by the inference worker (10 ms at the
/// 48 kHz engine rate; matches the CLI comparison block size).
pub const WORKER_BLOCK_SAMPLES: usize = 480;

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
#[derive(Debug)]
pub struct Runtime;

#[cfg(not(target_os = "macos"))]
impl Runtime {
    /// Rejects startup on non-macOS targets.
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

    /// Portable builds never receive audio callbacks.
    #[must_use]
    pub const fn frames_processed(&self) -> u64 {
        0
    }
}
