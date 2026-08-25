//! Speaker-embedding extraction for TSE enrollment.
//!
//! Reproduces the exact feature pipeline the tse-conv-tasnet-48k model was
//! trained with (SpeechBrain `Fbank` for the ECAPA preset), feeding the
//! author's own ECAPA-TDNN ONNX export
//! (`penta2himajin/ecapa-tdnn-onnx`, `features [1, T, 80] → embedding
//! [1, 192]`). Adapted from mellonella's `features.rs`/`embedding.rs`
//! (Apache-2.0; see `THIRD_PARTY_NOTICES.md`). The mel filterbank matrix is
//! vendored as a binary asset dumped from SpeechBrain so the projection is
//! bit-identical to upstream rather than re-derived.
//!
//! Pipeline: STFT (`n_fft` = win = 400, hop = 160, **periodic** Hamming
//! window, `center=True` zero padding) → power spectrum → mel projection
//! (vendored `(201, 80)` matrix) → `10·log10(max(1e-10, x))` → per-clip
//! `top_db = 80` floor.

use std::path::Path;

use noican_core::StageError;
use ort::session::Session;
use ort::value::Tensor;

use crate::dsp::FftPair;
use crate::onnx::{inference_error, load_streaming_session};

/// Embedding dimensionality.
pub const EMBEDDING_DIM: usize = 192;
/// Sample rate the feature pipeline expects.
pub const SAMPLE_RATE: u32 = 16_000;

const N_FFT: usize = 400;
const HOP: usize = 160;
const N_STFT: usize = N_FFT / 2 + 1;
const N_MELS: usize = 80;
const AMIN: f32 = 1e-10;
const TOP_DB: f32 = 80.0;

/// SpeechBrain ECAPA mel filterbank, `(N_STFT, N_MELS)` row-major f32 LE,
/// dumped from `speechbrain.lobes.features.Fbank` (via mellonella's
/// `scripts/dump_fbank_fixture.py`).
static FILTERBANK_BYTES: &[u8] = include_bytes!("../assets/fbank_filterbank.bin");

fn filterbank() -> Vec<f32> {
    let (chunks, _) = FILTERBANK_BYTES.as_chunks::<4>();
    chunks.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

/// SpeechBrain-compatible log-mel feature extractor.
#[derive(Debug)]
pub struct Fbank {
    fft: FftPair,
    window: Vec<f32>,
    /// `(N_STFT, N_MELS)` row-major.
    weights: Vec<f32>,
}

impl Default for Fbank {
    fn default() -> Self {
        Self::new()
    }
}

impl Fbank {
    /// Builds the extractor with the vendored SpeechBrain filterbank.
    ///
    /// # Panics
    ///
    /// Panics when the vendored filterbank asset is corrupt (build-time
    /// invariant).
    #[must_use]
    pub fn new() -> Self {
        let weights = filterbank();
        assert_eq!(weights.len(), N_STFT * N_MELS, "corrupt filterbank asset");
        // torch.hamming_window default is periodic: divisor N, not N - 1.
        #[allow(
            clippy::cast_precision_loss,
            reason = "window length 400 is exactly representable"
        )]
        let denom = N_FFT as f32;
        let window = (0..N_FFT)
            .map(|i| {
                #[allow(clippy::cast_precision_loss, reason = "window index is tiny")]
                let x = i as f32;
                0.46f32.mul_add(-(2.0 * std::f32::consts::PI * x / denom).cos(), 0.54)
            })
            .collect();
        Self {
            fft: FftPair::new(N_FFT),
            window,
            weights,
        }
    }

    /// Computes `(n_frames × N_MELS)` row-major log-mel features for
    /// 16 kHz `audio`. Frame count follows torch's `center=True` rule:
    /// `1 + len / hop`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on an internal FFT failure.
    pub fn compute(&mut self, audio: &[f32]) -> Result<Vec<f32>, StageError> {
        let pad = N_FFT / 2;
        let n_frames = 1 + audio.len() / HOP;
        let mut out = vec![0.0_f32; n_frames * N_MELS];
        let mut frame = vec![0.0_f32; N_FFT];
        let mut spec = vec![0.0_f32; 2 * N_STFT];

        let (rows, _) = out.as_chunks_mut::<N_MELS>();
        for (f_idx, row) in rows.iter_mut().enumerate() {
            let start_padded = f_idx * HOP;
            for (i, slot) in frame.iter_mut().enumerate() {
                let abs_idx = start_padded + i;
                let sample = if abs_idx < pad || abs_idx >= pad + audio.len() {
                    0.0
                } else {
                    audio[abs_idx - pad]
                };
                *slot = sample * self.window[i];
            }
            self.fft.forward_interleaved(&frame, &mut spec)?;
            let (weight_rows, _) = self.weights.as_chunks::<N_MELS>();
            for (b, weights_row) in weight_rows.iter().enumerate() {
                let re = spec[2 * b];
                let im = spec[2 * b + 1];
                let power = re.mul_add(re, im * im);
                for (acc, w) in row.iter_mut().zip(weights_row) {
                    *acc = power.mul_add(*w, *acc);
                }
            }
        }

        // 10 * log10(max(amin, x)) with a per-clip top_db floor.
        let mut max_db = f32::NEG_INFINITY;
        for v in &mut out {
            *v = 10.0 * v.max(AMIN).log10();
            max_db = max_db.max(*v);
        }
        let floor = max_db - TOP_DB;
        for v in &mut out {
            *v = v.max(floor);
        }
        Ok(out)
    }
}

/// ECAPA-TDNN embedding extractor (fbank + ONNX session).
#[derive(Debug)]
pub struct EcapaEmbedder {
    session: Session,
    fbank: Fbank,
}

impl EcapaEmbedder {
    /// Loads the ECAPA ONNX at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] when the model cannot be loaded.
    pub fn new(path: &Path) -> Result<Self, StageError> {
        Ok(Self {
            session: load_streaming_session(path)?,
            fbank: Fbank::new(),
        })
    }

    /// Computes the 192-dim speaker embedding of a 16 kHz mono clip
    /// (recommended length: 3–10 s of clean speech from the target
    /// speaker).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on feature or inference failure.
    pub fn embed(&mut self, audio_16k: &[f32]) -> Result<Vec<f32>, StageError> {
        let features = self.fbank.compute(audio_16k)?;
        let n_frames = features.len() / N_MELS;
        let tensor = Tensor::from_array(([1, n_frames, N_MELS], features))
            .map_err(|e| inference_error("building features tensor", &e))?;
        let outputs = self
            .session
            .run(ort::inputs!["features" => tensor])
            .map_err(|e| inference_error("ECAPA inference", &e))?;
        let (_, data) = outputs
            .get("embedding")
            .ok_or_else(|| StageError::Inference("embedding output missing".to_owned()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| inference_error("extracting embedding", &e))?;
        if data.len() != EMBEDDING_DIM {
            return Err(StageError::BufferLen {
                expected: EMBEDDING_DIM,
                got: data.len(),
            });
        }
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden parity against the Python SpeechBrain reference: the input
    /// and expected feature fixtures are dumped by mellonella's
    /// `scripts/dump_fbank_fixture.py` (1 s @ 16 kHz harmonic stack).
    #[test]
    fn fbank_matches_speechbrain_reference() {
        let input: Vec<f32> = include_bytes!("../tests/fixtures/fbank_input.bin")
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let expected: Vec<f32> = include_bytes!("../tests/fixtures/fbank_expected.bin")
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(input.len(), 16_000);
        assert_eq!(expected.len(), 101 * N_MELS);

        let mut fbank = Fbank::new();
        let got = fbank.compute(&input).expect("fbank");
        assert_eq!(got.len(), expected.len());
        let max_err = got
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        // Observed max |Δ| ≈ 1.2e-3 dB against the float32 Python dump —
        // fused multiply-adds round differently from the reference, which
        // is far below anything audible or embedding-relevant.
        assert!(max_err < 5e-3, "max |Δ| = {max_err}");
    }
}
