//! Shared machinery for `DeepFilterNet`-family models.
//!
//! One front-end serves both `DeepFilterNet3` and Hush, because they ship the
//! same three-graph bundle: `enc.onnx`, `erb_dec.onnx`, `df_dec.onnx`, and a
//! `config.ini` giving the transform and filter parameters.
//!
//! This whole module was validated against `DeepFilterNet`'s own inference
//! runtime rather than against its prose. Running Hush through both gives 0.999
//! correlation and agreeing per-band responses, which is what makes the
//! conventions in [`filter`] trustworthy — none of them is documented upstream.

pub mod config;
pub mod features;
pub mod filter;

pub use config::DfnConfig;
pub use features::{DfnFeatures, erb_band_widths};
pub use filter::{apply_band_gains, apply_deep_filter};
