//! Model registry, weight fetching, and ONNX-based stage implementations.
//!
//! Every supported model is described by a [`manifest::ModelSpec`] and
//! implemented as a [`noican_core::FrameProcessor`], wrapped into a
//! [`noican_core::Stage`] by the factory so the engine and CLI can treat all
//! models uniformly (docs/tech-research.md §12).

pub mod factory;
pub mod fetch;
pub mod manifest;
pub mod onnx;

pub use factory::{PASSTHROUGH_ID, create_stage};
pub use manifest::{ALL_MODELS, FileSpec, ModelFamily, ModelSpec};
