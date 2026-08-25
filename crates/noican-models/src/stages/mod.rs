//! Stage implementations, one per ONNX signature in the catalog.

pub mod spectral;
pub mod waveform;

pub use spectral::SpectralStage;
pub use waveform::WaveformStage;
