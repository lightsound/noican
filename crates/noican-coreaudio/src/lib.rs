//! See docs/adr/0003-coreaudio-transport.md.

use thiserror::Error;

#[cfg(not(target_os = "macos"))]
use std::sync::Arc;

#[cfg(not(target_os = "macos"))]
use noican_core::SwitchingEngine;

pub mod callback;
pub mod monitor;
pub mod observe;
pub mod routing;

pub use observe::{StreamLevels, WorkerBlockStats};
pub use routing::VirtualOutputChannels;

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

/// See docs/adr/0001-realtime-audio-rules.md.
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
    /// See docs/adr/0005-preview-monitor.md.
    #[error("preview monitor failed: {0}")]
    Monitor(String),
    /// See docs/adr/0005-preview-monitor.md.
    #[error("the system output is a virtual loopback device")]
    MonitorLoopbackOutput {
        /// UID of the rejected default output device.
        uid: String,
    },
    /// See docs/adr/0005-preview-monitor.md.
    #[error("the system output is a multi-output device")]
    MonitorAggregateOutput {
        /// UID of the rejected default output device.
        uid: String,
    },
    /// See docs/adr/0005-preview-monitor.md.
    #[error("the built-in speakers would feed back")]
    MonitorSpeakerOutput,
    /// See docs/adr/0004-aggregate-output-routing.md.
    #[error("virtual output routing failed: {0}")]
    OutputRouting(String),
    /// This build does not target macOS.
    #[error("AUHAL is available only on macOS")]
    UnsupportedPlatform,
}

#[cfg(not(target_os = "macos"))]
/// See docs/adr/0015-device-session.md.
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
        _virtual_output: VirtualOutputChannels,
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

    /// Portable builds never route a render stream.
    #[must_use]
    pub const fn routing_description(&self) -> &'static str {
        ""
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

    /// Portable builds never render, so they never underrun.
    #[must_use]
    pub const fn output_underruns(&self) -> u64 {
        0
    }

    /// Portable builds never capture, so they never overrun the input
    /// ring.
    #[must_use]
    pub const fn input_overruns(&self) -> u64 {
        0
    }

    /// Portable builds never run the inference worker.
    #[must_use]
    pub const fn worker_blocks(&self) -> u64 {
        0
    }

    /// Portable builds never run the inference worker.
    #[must_use]
    pub const fn worker_blocks_over_budget(&self) -> u64 {
        0
    }

    /// Portable builds never run the inference worker.
    #[must_use]
    pub const fn worker_block_max_ns(&self) -> u64 {
        0
    }

    /// Portable builds never run the inference worker, so it is never
    /// promoted to real-time scheduling.
    #[must_use]
    pub const fn worker_realtime(&self) -> bool {
        false
    }

    /// No-op portable reset of the diagnostic counters.
    pub const fn reset_debug_stats(&self) {}
}
