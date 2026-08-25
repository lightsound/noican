//! Stage implementations, one per ONNX signature in the catalog.

pub mod deep_filter_net;
pub mod spectral;
pub mod waveform;

pub use deep_filter_net::DeepFilterNetStage;
pub use spectral::SpectralStage;
pub use waveform::WaveformStage;
