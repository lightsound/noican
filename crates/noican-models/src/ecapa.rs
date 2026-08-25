//! `SpeechBrain` ECAPA-TDNN enrollment embedding.
//!
//! The feature extraction follows the Apache-2.0 mellonella implementation,
//! which in turn was parity-tested against `SpeechBrain`'s public
//! `spkrec-ecapa-voxceleb` model.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ndarray::Array3;
use ort::{session::Session, value::TensorRef};
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use thiserror::Error;

use crate::{assets::ModelAsset, tse::EMBEDDING_DIMENSIONS};

/// Sample rate required for ECAPA enrollment audio.
pub const SAMPLE_RATE: u32 = 16_000;
/// Assets required by the enrollment extractor.
pub const ASSETS: [ModelAsset; 2] = [ModelAsset::Ecapa, ModelAsset::EcapaFilterbank];

const MEL_BANDS: usize = 80;
const FFT_SIZE: usize = 400;
const HOP_SIZE: usize = 160;
const SPECTRUM_BINS: usize = FFT_SIZE / 2 + 1;
const MINIMUM_SAMPLES: usize = 16_000;

/// SpeechBrain-compatible ECAPA embedding extractor.
pub struct Ecapa {
    session: Session,
    fbank: Fbank,
}

impl Ecapa {
    /// Load the embedding graph and its pinned filterbank table.
    ///
    /// # Errors
    ///
    /// Returns [`EnrollmentError`] for I/O, table-shape, or ONNX failures.
    pub fn load(
        model_path: impl AsRef<Path>,
        filterbank_path: impl AsRef<Path>,
    ) -> Result<Self, EnrollmentError> {
        let session = Session::builder()
            .map_err(|error| EnrollmentError::Onnx(error.to_string()))?
            .with_intra_threads(1)
            .map_err(|error| EnrollmentError::Onnx(error.to_string()))?
            .with_inter_threads(1)
            .map_err(|error| EnrollmentError::Onnx(error.to_string()))?
            .commit_from_file(model_path)
            .map_err(|error| EnrollmentError::Onnx(error.to_string()))?;
        let fbank = Fbank::load(filterbank_path)?;
        Ok(Self { session, fbank })
    }

    /// Compute the 192-dimensional TSE conditioning vector from mono 16 kHz audio.
    ///
    /// # Errors
    ///
    /// Returns [`EnrollmentError::TooShort`] below one second and propagates
    /// feature or ONNX failures.
    pub fn embed(&mut self, audio: &[f32]) -> Result<[f32; EMBEDDING_DIMENSIONS], EnrollmentError> {
        if audio.len() < MINIMUM_SAMPLES {
            return Err(EnrollmentError::TooShort {
                actual: audio.len(),
                minimum: MINIMUM_SAMPLES,
            });
        }
        if audio.iter().any(|sample| !sample.is_finite()) {
            return Err(EnrollmentError::NonFiniteAudio);
        }
        let features = self.fbank.compute(audio);
        let frame_count = features.len() / MEL_BANDS;
        let features = Array3::from_shape_vec((1, frame_count, MEL_BANDS), features)
            .map_err(|error| EnrollmentError::Shape(error.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs![
                "features" => TensorRef::from_array_view(&features)
                    .map_err(|error| EnrollmentError::Onnx(error.to_string()))?
            ])
            .map_err(|error| EnrollmentError::Onnx(error.to_string()))?;
        let (shape, values) = outputs["embedding"]
            .try_extract_tensor::<f32>()
            .map_err(|error| EnrollmentError::Onnx(error.to_string()))?;
        if values.len() != EMBEDDING_DIMENSIONS {
            return Err(EnrollmentError::UnexpectedEmbedding {
                shape: shape.to_vec(),
            });
        }
        let mut embedding = [0.0_f32; EMBEDDING_DIMENSIONS];
        embedding.copy_from_slice(values);
        if embedding.iter().any(|value| !value.is_finite()) {
            return Err(EnrollmentError::NonFiniteEmbedding);
        }
        Ok(embedding)
    }
}

struct Fbank {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    window: [f32; FFT_SIZE],
    filterbank: Box<[f32]>,
}

impl Fbank {
    fn load(path: impl AsRef<Path>) -> Result<Self, EnrollmentError> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| EnrollmentError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let expected = SPECTRUM_BINS * MEL_BANDS * size_of::<f32>();
        if bytes.len() != expected {
            return Err(EnrollmentError::FilterbankSize {
                actual: bytes.len(),
                expected,
            });
        }
        let mut filterbank = Vec::with_capacity(SPECTRUM_BINS * MEL_BANDS);
        for bytes in bytes.chunks_exact(size_of::<f32>()) {
            filterbank.push(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
        }
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch = vec![Complex32::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Ok(Self {
            fft,
            scratch,
            window: hamming_window(),
            filterbank: filterbank.into_boxed_slice(),
        })
    }

    fn compute(&mut self, audio: &[f32]) -> Vec<f32> {
        const MINIMUM_POWER: f32 = 1e-10;
        const TOP_DB: f32 = 80.0;
        let padding = FFT_SIZE / 2;
        let frame_count = 1 + audio.len() / HOP_SIZE;
        let mut output = vec![0.0_f32; frame_count * MEL_BANDS];
        let mut frame = vec![Complex32::new(0.0, 0.0); FFT_SIZE];
        let mut power = [0.0_f32; SPECTRUM_BINS];

        for frame_index in 0..frame_count {
            let padded_start = frame_index * HOP_SIZE;
            for (index, value) in frame.iter_mut().enumerate() {
                let padded_index = padded_start + index;
                let sample = if padded_index < padding || padded_index >= padding + audio.len() {
                    0.0
                } else {
                    audio[padded_index - padding]
                };
                *value = Complex32::new(sample * self.window[index], 0.0);
            }
            self.fft.process_with_scratch(&mut frame, &mut self.scratch);
            for (index, value) in power.iter_mut().enumerate() {
                *value = frame[index].norm_sqr();
            }
            let row = &mut output[frame_index * MEL_BANDS..(frame_index + 1) * MEL_BANDS];
            for (bin, bin_power) in power.iter().enumerate() {
                let weights = &self.filterbank[bin * MEL_BANDS..(bin + 1) * MEL_BANDS];
                for (value, weight) in row.iter_mut().zip(weights) {
                    *value += bin_power * weight;
                }
            }
        }

        let mut maximum = f32::NEG_INFINITY;
        for value in &mut output {
            *value = 10.0 * value.max(MINIMUM_POWER).log10();
            maximum = maximum.max(*value);
        }
        let floor = maximum - TOP_DB;
        for value in &mut output {
            *value = value.max(floor);
        }
        output
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "the fixed 400-point periodic Hamming window is exactly the f32 SpeechBrain model input contract"
)]
fn hamming_window() -> [f32; FFT_SIZE] {
    let mut window = [0.0_f32; FFT_SIZE];
    for (index, value) in window.iter_mut().enumerate() {
        *value = 0.46_f64.mul_add(
            -(2.0 * std::f64::consts::PI * index as f64 / FFT_SIZE as f64).cos(),
            0.54,
        ) as f32;
    }
    window
}

/// Enrollment extraction failures.
#[derive(Debug, Error)]
pub enum EnrollmentError {
    /// Enrollment clip is shorter than the model minimum.
    #[error("enrollment requires at least {minimum} samples, received {actual}")]
    TooShort {
        /// Supplied sample count.
        actual: usize,
        /// Required sample count.
        minimum: usize,
    },
    /// Input contains a NaN or infinity.
    #[error("enrollment audio contains a non-finite sample")]
    NonFiniteAudio,
    /// Model output contains a NaN or infinity.
    #[error("ECAPA returned a non-finite embedding")]
    NonFiniteEmbedding,
    /// Filterbank table has the wrong byte count.
    #[error("ECAPA filterbank contains {actual} bytes, expected {expected}")]
    FilterbankSize {
        /// Supplied byte count.
        actual: usize,
        /// Required byte count.
        expected: usize,
    },
    /// Filesystem operation failed.
    #[error("I/O error at {}: {source}", path.display())]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// ndarray rejected a tensor shape.
    #[error("ECAPA tensor shape failed: {0}")]
    Shape(String),
    /// ONNX Runtime failed.
    #[error("ECAPA ONNX failed: {0}")]
    Onnx(String),
    /// Output tensor has the wrong width.
    #[error("ECAPA returned an unexpected embedding shape {shape:?}")]
    UnexpectedEmbedding {
        /// Runtime shape.
        shape: Vec<i64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hamming_window_is_finite_and_periodic() {
        let window = hamming_window();
        assert!(window.iter().all(|value| value.is_finite()));
        assert!(window[0] > 0.0);
        assert!(window[FFT_SIZE - 1] > window[0]);
    }
}
