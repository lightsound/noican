//! Inference backends and verified model assets for noican.

pub mod assets;
mod deep_filter;
mod dpdfnet;
mod dsp;
pub mod ecapa;
mod fastenhancer;
mod registry;
mod tse;
mod ul_unas;

pub use registry::{load_pipeline_stage, LoadRequest, ModelId, ModelLoadError, UnknownModel};
pub use tse::EMBEDDING_DIMENSIONS;
