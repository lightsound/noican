//! Real-time engine: physical mic → selected model → BlackHole.
//!
//! macOS implementation notes (docs/tech-research.md §4):
//!
//! - Core Audio HAL is driven **directly** (`AudioDeviceCreateIOProcID` on
//!   a device we own) — `AVAudioEngine` cannot target non-default HAL
//!   devices and is not used anywhere.
//! - The physical microphone and the virtual output device live in
//!   different clock domains; they are combined into a **private aggregate
//!   device** with drift compensation enabled on the output sub-device, so
//!   one IOProc delivers time-aligned input and output buffers.
//! - The IOProc (audio thread) does nothing but move samples between the
//!   hardware buffers and wait-free SPSC rings (§9: no allocation, no
//!   locks, no inference on the audio thread). A dedicated inference
//!   thread drains the input ring, runs the active stage through
//!   [`noican_core::StageSwitcher`] (lock-free model switching with a
//!   click-free crossfade), and refills the output ring.
//!
//! Everything platform-specific is `#[cfg(target_os = "macos")]`; the
//! crate compiles (with the engine unavailable) on other platforms so the
//! workspace-wide quality gates run everywhere.

pub mod engine;

#[cfg(target_os = "macos")]
pub mod coreaudio;

pub use engine::{DeviceInfo, EngineConfig, EngineStatus, RtEngine, RtError};
