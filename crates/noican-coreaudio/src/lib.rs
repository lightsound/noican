//! AUHAL transport between Core Audio devices and the Rust engine.
//!
//! Ported from the Phase 0 transport candidate that passed hardware
//! acceptance on macOS 26 / Apple Silicon (mic permission → Running,
//! QuickTime recording through `BlackHole` 2ch, clean teardown), rewired to
//! drive [`noican_core::SwitchingEngine`] so the processing path — including
//! the 16 kHz polyphase resampling in [`noican_core::FramedStage`] — is the
//! exact code the CLI comparison mode exercises.
//!
//! Two transport shapes exist: one AUHAL on a private Aggregate Device
//! for 48 kHz-capable microphones (the original path, unchanged), and a
//! split transport — a native-rate capture AUHAL plus a 48 kHz virtual-
//! output AUHAL bridged by a drift-compensating resampler — for
//! telephony-profile microphones such as Bluetooth headsets (issue #7).
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
use std::sync::Arc;

#[cfg(not(target_os = "macos"))]
use noican_core::SwitchingEngine;

pub mod aec;
pub mod monitor;
pub mod observe;

pub use observe::StreamLevels;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{Runtime, check_monitor_device, check_monitor_target};

/// Portable stub of the preview-target pre-flight check.
///
/// # Errors
///
/// Always returns [`CoreAudioError::UnsupportedPlatform`].
#[cfg(not(target_os = "macos"))]
pub const fn check_monitor_target() -> Result<(), CoreAudioError> {
    Err(CoreAudioError::UnsupportedPlatform)
}

/// Portable stub of the per-device preview-target check.
///
/// # Errors
///
/// Always returns [`CoreAudioError::UnsupportedPlatform`].
#[cfg(not(target_os = "macos"))]
pub const fn check_monitor_device(_device: u32) -> Result<(), CoreAudioError> {
    Err(CoreAudioError::UnsupportedPlatform)
}

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
    /// A control call needs a running transport.
    #[error("the engine transport is not running")]
    NotRunning,
    /// The preview monitor path could not start. Monitor failures never
    /// affect the meeting-facing path.
    #[error("preview monitor failed: {0}")]
    Monitor(String),
    /// Enabling preview was refused because the system default output is a
    /// virtual loopback device, which would feed the processed voice into
    /// the meeting a second time.
    ///
    /// The message is one short cause (no UID, no remedy): the UI composes
    /// it into its own sentences ("Preview needs headphones — …",
    /// "Preview stopped: …"), and the rejected device's UID is kept in
    /// the variant for programmatic callers.
    #[error("the system output is a virtual loopback device")]
    MonitorLoopbackOutput {
        /// UID of the rejected default output device.
        uid: String,
    },
    /// Enabling preview was refused because the system default output is
    /// an aggregate or Multi-Output device, which can contain the meeting
    /// loopback as a subdevice this check cannot cheaply inspect. Message
    /// shape: see [`CoreAudioError::MonitorLoopbackOutput`].
    #[error("the system output is a multi-output device")]
    MonitorAggregateOutput {
        /// UID of the rejected default output device.
        uid: String,
    },
    /// Enabling preview was refused because the system default output is
    /// the built-in speakers, which would feed the processed microphone
    /// straight back into itself (Phase 0/1 has no echo cancellation).
    /// Message shape: see [`CoreAudioError::MonitorLoopbackOutput`].
    #[error("the built-in speakers would feed back")]
    MonitorSpeakerOutput,
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
    pub fn start(
        _aggregate_device: u32,
        _engine: SwitchingEngine,
        _levels: Arc<StreamLevels>,
        _monitor_state: Arc<std::sync::atomic::AtomicI32>,
    ) -> Result<Self, CoreAudioError> {
        Err(CoreAudioError::UnsupportedPlatform)
    }

    /// Rejects split-transport startup on non-macOS targets.
    ///
    /// # Errors
    ///
    /// Always returns [`CoreAudioError::UnsupportedPlatform`].
    pub fn start_native(
        _input_device: u32,
        _output_device: u32,
        _capture_rate: u32,
        _engine: SwitchingEngine,
        _levels: Arc<StreamLevels>,
        _monitor_state: Arc<std::sync::atomic::AtomicI32>,
    ) -> Result<Self, CoreAudioError> {
        Err(CoreAudioError::UnsupportedPlatform)
    }

    /// No-op portable shutdown.
    pub const fn stop(&mut self) {}

    /// Rejects preview monitoring on non-macOS targets.
    ///
    /// # Errors
    ///
    /// Always returns [`CoreAudioError::UnsupportedPlatform`].
    pub const fn set_monitor(&mut self, _enabled: bool) -> Result<(), CoreAudioError> {
        Err(CoreAudioError::UnsupportedPlatform)
    }

    /// Portable builds never have a monitor device.
    #[must_use]
    pub const fn monitor_device(&self) -> Option<u32> {
        None
    }

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
