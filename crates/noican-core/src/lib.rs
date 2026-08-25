//! Real-time-safe audio primitives and the processing-stage abstraction shared
//! by every part of noican.
//!
//! This crate deliberately knows nothing about neural networks, Core Audio, or
//! file formats. It defines the contract that every enhancement model has to
//! satisfy ([`Stage`]) plus the DSP needed to run a stage whose native sample
//! rate and block size differ from the host's ([`StageRunner`]).
//!
//! # Real-time rules
//!
//! Everything reachable from [`Stage::process`] and [`StageRunner::process`] is
//! written to the constraints in `docs/tech-research.md` §9: no heap allocation,
//! no locks, no logging, no file or system calls. Allocation happens only in
//! constructors, which are called from the control plane.

pub mod error;
pub mod resample;
pub mod ring;
pub mod runner;
pub mod stage;
pub mod stft;
pub mod window;

pub use error::{Error, Result};
pub use resample::RationalResampler;
pub use ring::SampleQueue;
pub use runner::StageRunner;
pub use stage::{Stage, StageCapability, StageSpec};
pub use stft::{Complex32, Spectrum, StftAnalyzer, StftConfig, StftSynthesizer};
pub use window::WindowKind;

/// Sample rate of the host audio path, in hertz.
///
/// The physical microphone, the aggregate device, and the virtual output device
/// all run at this rate; per-model rate conversion is handled by
/// [`StageRunner`].
pub const HOST_SAMPLE_RATE: u32 = 48_000;
