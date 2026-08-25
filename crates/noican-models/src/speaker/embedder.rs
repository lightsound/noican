//! Turning a window of audio into a speaker embedding.

use std::path::Path;

use ort::session::Session;
use ort::value::TensorRef;

use super::fbank::{LogMelFbank, MEL_BANDS, SAMPLE_RATE};
use crate::error::{Error, Result};

/// Shortest window that produces a usable embedding, in seconds.
///
/// Measured, not chosen. Separation between same-speaker and different-speaker
/// pairs on labelled data, as the window shrinks:
///
/// | Window | Same-speaker | Different-speaker | Worst-case margin |
/// |---|---|---|---|
/// | 0.5 s | 0.162 | 0.024 | −0.472 |
/// | 1.0 s | 0.266 | 0.020 | −0.163 |
/// | 1.5 s | 0.423 | 0.015 | +0.149 |
/// | 2.0 s | 0.376 | −0.002 | +0.231 |
///
/// Different-speaker scores stay near zero at every length; it is the
/// same-speaker score that falls apart. Below about 1.5 seconds the model cannot
/// recognise anyone, so a gate built on shorter windows would reject its own
/// user. Hence the minimum, and hence the fact that this gate reacts in seconds
/// rather than milliseconds.
pub const MINIMUM_WINDOW_SECONDS: f32 = 1.5;

/// Extracts speaker embeddings from 16 kHz mono audio.
pub struct SpeakerEmbedder {
    model_id: String,
    session: Session,
    fbank: LogMelFbank,
    features: Vec<f32>,
    dimension: usize,
}

impl std::fmt::Debug for SpeakerEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeakerEmbedder")
            .field("model_id", &self.model_id)
            .field("dimension", &self.dimension)
            .finish_non_exhaustive()
    }
}

impl SpeakerEmbedder {
    /// Loads the embedding graph at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Runtime`] if the graph cannot be loaded and
    /// [`Error::UnexpectedSignature`] if it does not take `[batch, frames,
    /// bands]` features.
    pub fn load(model_id: &str, path: &Path) -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(1)
            .map_err(ort::Error::from)?
            .commit_from_file(path)?;

        let input = session
            .inputs()
            .first()
            .ok_or_else(|| Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: "the graph takes no inputs".to_owned(),
            })?;
        let bands = input
            .dtype()
            .tensor_shape()
            .and_then(|shape| shape.last().copied());
        if bands != Some(i64::try_from(MEL_BANDS).unwrap_or(-1)) {
            return Err(Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: format!(
                    "expected features with {MEL_BANDS} bands on the trailing axis, graph declares \
                     {bands:?}"
                ),
            });
        }

        let dimension = session
            .outputs()
            .first()
            .and_then(|output| output.dtype().tensor_shape())
            .and_then(|shape| shape.last().copied())
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| Error::UnexpectedSignature {
                model: model_id.to_owned(),
                detail: "the graph does not declare its embedding dimension".to_owned(),
            })?;

        Ok(Self {
            model_id: model_id.to_owned(),
            session,
            fbank: LogMelFbank::new(),
            features: Vec::new(),
            dimension,
        })
    }

    /// Dimension of the embeddings this model produces.
    #[must_use]
    pub const fn dimension(&self) -> usize {
        self.dimension
    }

    /// Shortest usable window, in samples at the model's rate.
    #[must_use]
    pub fn minimum_window_samples() -> usize {
        #[expect(
            clippy::cast_precision_loss,
            reason = "audio sample rates are exact in f32"
        )]
        let rate = SAMPLE_RATE as f32;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the product is a small positive sample count"
        )]
        let samples = (rate * MINIMUM_WINDOW_SECONDS) as usize;
        samples
    }

    /// Embeds one window of 16 kHz mono audio.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enrolment`] if the window is shorter than one frame and
    /// [`Error::Runtime`] if inference fails.
    pub fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let frames = self.fbank.compute(samples, &mut self.features);
        if frames == 0 {
            return Err(Error::Enrolment {
                detail: format!(
                    "a {}-sample window is shorter than one frame",
                    samples.len()
                ),
            });
        }

        let shape = [
            1i64,
            i64::try_from(frames).unwrap_or(i64::MAX),
            i64::try_from(MEL_BANDS).unwrap_or(0),
        ];
        let features = TensorRef::from_array_view((&shape[..], self.features.as_slice()))?;
        let outputs = self.session.run(ort::inputs![features])?;
        let (_, embedding) = outputs[0].try_extract_tensor::<f32>()?;

        if embedding.len() != self.dimension {
            return Err(Error::UnexpectedSignature {
                model: self.model_id.clone(),
                detail: format!(
                    "expected a {}-dimensional embedding, got {}",
                    self.dimension,
                    embedding.len()
                ),
            });
        }
        Ok(embedding.to_vec())
    }

    /// Embeds `samples` in overlapping windows, one embedding per window.
    ///
    /// Windows step by half their length so a speaker change cannot fall
    /// entirely between two of them. Returns an empty vector when the input is
    /// shorter than one window.
    ///
    /// # Errors
    ///
    /// Propagates inference failures from [`Self::embed`].
    pub fn embed_windows(&mut self, samples: &[f32]) -> Result<Vec<Vec<f32>>> {
        let window = Self::minimum_window_samples();
        let step = window / 2;
        let mut embeddings = Vec::new();
        let mut start = 0;
        while start + window <= samples.len() {
            embeddings.push(self.embed(&samples[start..start + window])?);
            start += step;
        }
        Ok(embeddings)
    }
}
