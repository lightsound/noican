//! Core audio-processing abstractions for the noican engine.
//!
//! This crate is platform-independent and free of inference-runtime
//! dependencies. It defines the common stage interface that every
//! noise-suppression / speaker-suppression model implements, plus the
//! sample-rate and frame-size adaptation that lets the engine treat all
//! models identically (docs/tech-research.md §12, Phase 0).
