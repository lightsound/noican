//! See docs/adr/0009-model-catalog-fetch.md#model-backend-selection.

pub mod dsp;
pub mod factory;
pub mod fetch;
pub mod manifest;
pub mod onnx;
pub mod stages;
pub mod traits;

pub use factory::{CatalogEntry, MAX_BLOCK_LEN, PASSTHROUGH_ID, catalog, create_stage};
pub use manifest::{ALL_MODELS, FileSpec, ModelFamily, ModelSpec};
pub use traits::ModelTraits;
