//! A stage that attenuates audio when the dominant speaker is not the enrolled
//! one.
//!
//! # What this can and cannot do
//!
//! The embedding model needs about 1.5 seconds of speech to recognise anyone
//! (see [`MINIMUM_WINDOW_SECONDS`]), so the gate decides from a sliding window
//! of recent audio and reacts in seconds. It suppresses a *sustained* other
//! voice — somebody else talking in the room while you are quiet. It will not
//! catch a single interjected word, and it is not a substitute for a model like
//! Hush, which separates overlapping speakers within a frame but cannot be told
//! who you are. The two are complementary.
//!
//! The decision and gain logic lives in [`crate::speaker::gate`], which is
//! testable without a model; this file is the plumbing around it.

use noican_core::{Result as CoreResult, Stage, StageCapability, StageSpec};

use crate::error::{Error, Result};
use crate::speaker::embedder::{MINIMUM_WINDOW_SECONDS, SpeakerEmbedder};
use crate::speaker::fbank::SAMPLE_RATE;
use crate::speaker::gate::{Gate, GateConfig, GateState};
use crate::speaker::profile::SpeakerProfile;

/// Samples processed per call. A tenth of a second at the model's rate.
const BLOCK_SIZE: usize = 1_600;

/// Level below which a window is treated as silence rather than as speech.
///
/// Without this, room noise between utterances embeds to something arbitrary and
/// the gate flaps. Silence holds the previous decision instead.
const SILENCE_RMS: f32 = 1e-3;

/// Gates on speaker identity against an enrolled profile.
pub struct SpeakerGateStage {
    embedder: SpeakerEmbedder,
    profile: SpeakerProfile,
    gate: Gate,
    spec: StageSpec,

    /// Sliding window of recent input, oldest first.
    window: Vec<f32>,
    window_capacity: usize,
    /// Samples accumulated since the last decision.
    since_decision: usize,
    decision_interval: usize,
}

impl std::fmt::Debug for SpeakerGateStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeakerGateStage")
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

impl SpeakerGateStage {
    /// Builds the gate from an embedding graph and an enrolled profile.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enrolment`] if the profile came from a different model
    /// or has the wrong dimension, and propagates load failures.
    pub fn new(
        model_id: &str,
        graph: &std::path::Path,
        profile: SpeakerProfile,
        latency_samples: usize,
    ) -> Result<Self> {
        let embedder = SpeakerEmbedder::load(model_id, graph)?;

        if profile.model_id != model_id {
            return Err(Error::Enrolment {
                detail: format!(
                    "the profile was enrolled with `{}` but the gate is running `{model_id}`; \
                     embeddings are not comparable across models, so re-enrol",
                    profile.model_id
                ),
            });
        }
        if profile.embedding.len() != embedder.dimension() {
            return Err(Error::Enrolment {
                detail: format!(
                    "the profile is {}-dimensional but the model produces {}",
                    profile.embedding.len(),
                    embedder.dimension()
                ),
            });
        }

        let window_capacity = SpeakerEmbedder::minimum_window_samples();
        Ok(Self {
            embedder,
            profile,
            gate: Gate::new(GateConfig::recommended(SAMPLE_RATE)),
            spec: StageSpec::streaming(SAMPLE_RATE, BLOCK_SIZE)
                .with_capability(StageCapability::Block)
                .with_latency(latency_samples),
            window: Vec::with_capacity(window_capacity),
            window_capacity,
            since_decision: 0,
            // Re-decide every quarter of a window, so the gate is never further
            // behind a change than that.
            decision_interval: window_capacity / 4,
        })
    }

    /// The gate's current state.
    #[must_use]
    pub const fn state(&self) -> GateState {
        self.gate.state()
    }

    /// Similarity from the most recent decision.
    #[must_use]
    pub const fn similarity(&self) -> f32 {
        self.gate.similarity()
    }

    /// Seconds of speech the gate needs before it can decide anything.
    #[must_use]
    pub const fn warm_up_seconds() -> f32 {
        MINIMUM_WINDOW_SECONDS
    }

    /// Re-evaluates the gate against the current window.
    fn decide(&mut self) {
        if self.window.len() < self.window_capacity {
            return;
        }

        #[expect(
            clippy::cast_precision_loss,
            reason = "window lengths are small integers, exact in f32"
        )]
        let count = self.window.len() as f32;
        let rms = (self.window.iter().map(|value| value * value).sum::<f32>() / count).sqrt();
        if rms < SILENCE_RMS {
            // Nobody is talking. Hold the previous decision rather than letting
            // room noise choose one.
            return;
        }

        if let Ok(embedding) = self.embedder.embed(&self.window) {
            let similarity = self.profile.similarity(&embedding);
            self.gate.observe(similarity);
        }
    }
}

impl Stage for SpeakerGateStage {
    fn spec(&self) -> StageSpec {
        self.spec
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> CoreResult<()> {
        if input.len() != BLOCK_SIZE || output.len() != BLOCK_SIZE {
            return Err(noican_core::Error::BufferLength {
                expected: BLOCK_SIZE,
                actual: input.len().min(output.len()),
            });
        }

        // Slide the window, keeping the newest audio, which is what the decision
        // is about.
        let overflow = (self.window.len() + input.len()).saturating_sub(self.window_capacity);
        if overflow > 0 {
            self.window.drain(..overflow.min(self.window.len()));
        }
        self.window.extend_from_slice(input);

        self.since_decision += input.len();
        if self.since_decision >= self.decision_interval {
            self.since_decision = 0;
            self.decide();
        }

        self.gate.apply(input, output);
        Ok(())
    }

    fn reset(&mut self) {
        self.window.clear();
        self.since_decision = 0;
        self.gate.reset();
    }
}
