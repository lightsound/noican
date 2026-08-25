//! Model registry, weight fetching, and stage implementations.
//!
//! Every supported model is described by a [`manifest::ModelSpec`] and
//! implemented as a [`noican_core::FrameProcessor`], wrapped into a
//! [`noican_core::Stage`] by [`factory::create_stage`] so the engine and
//! CLI can treat all models uniformly (docs/tech-research.md §12).
//!
//! Inference backends: ONNX Runtime (`ort`) for FastEnhancer, DPDFNet,
//! UL-UNAS, TSE, and ECAPA; tract (via the upstream `deep_filter` crate)
//! for the DeepFilterNet-architecture models (DeepFilterNet3, Hush),
//! whose three-graph exports are only streamable through tract-pulse.

pub mod dsp;
pub mod embedding;
pub mod factory;
pub mod fetch;
pub mod manifest;
pub mod onnx;
pub mod stages;

pub use factory::{MAX_BLOCK_LEN, PASSTHROUGH_ID, StageOptions, create_stage};
pub use manifest::{ALL_MODELS, FileSpec, ModelFamily, ModelSpec};
