//! Inference backends and verified model assets for noican.

pub mod assets;
pub mod deep_filter;
pub mod dpdfnet;
pub mod dsp;
pub mod ecapa;
pub mod fastenhancer;
pub mod registry;
pub mod tse;
pub mod ul_unas;

pub use registry::{load_pipeline_stage, LoadRequest, ModelId, ModelLoadError, UnknownModel};
pub use tse::EMBEDDING_DIMENSIONS;
