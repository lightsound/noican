//! Speaker recognition: log-mel features, embeddings, and the enrolled profile.
//!
//! These exist to answer one question at run time — is the person talking the
//! one who enrolled? — which is what the speaker gate in
//! [`crate::stages::speaker_gate`] acts on.
//!
//! The published export documents nothing about its features, so every choice
//! here was settled by measuring whether the resulting embeddings separate
//! labelled speakers. That search is recorded in [`fbank`] and
//! [`embedder::MINIMUM_WINDOW_SECONDS`]; the short version is that decibel-scaled
//! bands and a window of at least 1.5 seconds are both load-bearing.

pub mod embedder;
pub mod fbank;
pub mod gate;
pub mod profile;

pub use embedder::SpeakerEmbedder;
pub use fbank::LogMelFbank;
pub use gate::{Gate, GateConfig, GateState};
pub use profile::{EMBEDDING_DIMENSION, PROFILE_FILE_NAME, SpeakerProfile};
