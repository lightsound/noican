//! Stateful `DeepFilterNet3` ONNX stage.
//!
//! The per-frame graph and lookahead protocol follow the Apache-2.0
//! mellonella export. The spectral front end is a focused Rust implementation
//! of the MIT/Apache-2.0 `libDF` formulas.

use std::{collections::VecDeque, path::Path, sync::Arc};

use ndarray::{Array3, Array4, Array5};
use noican_engine::{
    validate_frame_lengths, AudioStage, EnrollmentRequirement, StageDescriptor, StageError,
    StageKind,
};
use ort::{session::Session, value::TensorRef};
use rustfft::{num_complex::Complex32, Fft, FftPlanner};

use crate::assets::ModelAsset;

const SAMPLE_RATE: u32 = 48_000;
const FFT_SIZE: usize = 960;
const FRAME_SAMPLES: usize = 480;
const SPECTRUM_BINS: usize = FFT_SIZE / 2 + 1;
const ERB_BANDS: usize = 32;
const DF_BINS: usize = 96;
const MIN_ERB_BINS: usize = 2;
const NORMALIZATION_ALPHA: f32 = 0.99;
const CONV_LOOKAHEAD: usize = 2;
const GRU_HIDDEN: usize = 256;
const ENCODER_LAYERS: usize = 1;
const ERB_LAYERS: usize = 2;
const DF_LAYERS: usize = 2;

const DESCRIPTOR: StageDescriptor = StageDescriptor {
    id: "deepfilternet3",
    display_name: "DeepFilterNet3",
    kind: StageKind::NoiseSuppression,
    sample_rate: SAMPLE_RATE,
    frame_samples: FRAME_SAMPLES,
    algorithmic_delay_samples: (CONV_LOOKAHEAD + 1) * FRAME_SAMPLES,
    tail_frames: CONV_LOOKAHEAD + 1,
    enrollment: EnrollmentRequirement::None,
};

/// Model file required by the `DeepFilterNet3` stage.
pub const ASSET: ModelAsset = ModelAsset::DeepFilterNet3;

/// Stateful `DeepFilterNet3` stage.
pub struct DeepFilterNet3 {
    session: Session,
    dsp: DeepFilterDsp,
    encoder_state: Vec<f32>,
    erb_state: Vec<f32>,
    df_state: Vec<f32>,
    spectra: VecDeque<Vec<f32>>,
    erb_features: VecDeque<Vec<f32>>,
    df_features: VecDeque<Vec<f32>>,
}

impl DeepFilterNet3 {
    /// Load the stateful per-frame ONNX graph.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Backend`] when ONNX Runtime rejects the graph.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, StageError> {
        let session = Session::builder()
            .map_err(backend_error)?
            .with_intra_threads(1)
            .map_err(backend_error)?
            .with_inter_threads(1)
            .map_err(backend_error)?
            .commit_from_file(path)
            .map_err(backend_error)?;
        Ok(Self {
            session,
            dsp: DeepFilterDsp::new(),
            encoder_state: vec![0.0; ENCODER_LAYERS * GRU_HIDDEN],
            erb_state: vec![0.0; ERB_LAYERS * GRU_HIDDEN],
            df_state: vec![0.0; DF_LAYERS * GRU_HIDDEN],
            spectra: VecDeque::with_capacity(CONV_LOOKAHEAD + 2),
            erb_features: VecDeque::with_capacity(CONV_LOOKAHEAD + 2),
            df_features: VecDeque::with_capacity(CONV_LOOKAHEAD + 2),
        })
    }

    fn infer_oldest(&mut self) -> Result<Vec<f32>, StageError> {
        let spectrum = self
            .spectra
            .pop_front()
            .ok_or_else(|| backend_error("missing queued spectrum"))?;
        let erb = self
            .erb_features
            .get(CONV_LOOKAHEAD)
            .ok_or_else(|| backend_error("missing lookahead ERB feature"))?
            .clone();
        let df = self
            .df_features
            .get(CONV_LOOKAHEAD)
            .ok_or_else(|| backend_error("missing lookahead DF feature"))?
            .clone();
        self.erb_features.pop_front();
        self.df_features.pop_front();

        let spectrum_input =
            Array5::from_shape_vec((1, 1, 1, SPECTRUM_BINS, 2), spectrum).map_err(backend_error)?;
        let erb_input = Array4::from_shape_vec((1, 1, 1, ERB_BANDS), erb).map_err(backend_error)?;
        let df_input = Array5::from_shape_vec((1, 1, 1, DF_BINS, 2), df).map_err(backend_error)?;
        let encoder_state =
            Array3::from_shape_vec((ENCODER_LAYERS, 1, GRU_HIDDEN), self.encoder_state.clone())
                .map_err(backend_error)?;
        let erb_state = Array3::from_shape_vec((ERB_LAYERS, 1, GRU_HIDDEN), self.erb_state.clone())
            .map_err(backend_error)?;
        let df_state = Array3::from_shape_vec((DF_LAYERS, 1, GRU_HIDDEN), self.df_state.clone())
            .map_err(backend_error)?;

        let (enhanced, encoder_state, erb_state, df_state) = {
            let outputs = self
                .session
                .run(ort::inputs![
                    "spec" => TensorRef::from_array_view(&spectrum_input)
                        .map_err(backend_error)?,
                    "feat_erb" => TensorRef::from_array_view(&erb_input)
                        .map_err(backend_error)?,
                    "feat_spec" => TensorRef::from_array_view(&df_input)
                        .map_err(backend_error)?,
                    "enc_h" => TensorRef::from_array_view(&encoder_state)
                        .map_err(backend_error)?,
                    "erb_h" => TensorRef::from_array_view(&erb_state)
                        .map_err(backend_error)?,
                    "df_h" => TensorRef::from_array_view(&df_state)
                        .map_err(backend_error)?,
                ])
                .map_err(backend_error)?;
            (
                output_values(&outputs, "enhanced_spec", SPECTRUM_BINS * 2)?,
                output_values(&outputs, "new_enc_h", ENCODER_LAYERS * GRU_HIDDEN)?,
                output_values(&outputs, "new_erb_h", ERB_LAYERS * GRU_HIDDEN)?,
                output_values(&outputs, "new_df_h", DF_LAYERS * GRU_HIDDEN)?,
            )
        };
        self.encoder_state = encoder_state;
        self.erb_state = erb_state;
        self.df_state = df_state;
        Ok(enhanced)
    }
}

impl AudioStage for DeepFilterNet3 {
    fn descriptor(&self) -> StageDescriptor {
        DESCRIPTOR
    }

    fn process_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
        validate_frame_lengths(DESCRIPTOR, input, output)?;
        let spectrum = self.dsp.analyze(input);
        let erb = self.dsp.erb_features(&spectrum);
        let df = self.dsp.df_features(&spectrum);
        self.spectra.push_back(flatten_complex(&spectrum));
        self.erb_features.push_back(erb);
        self.df_features.push_back(df);
        if self.spectra.len() <= CONV_LOOKAHEAD {
            output.fill(0.0);
            return Ok(());
        }
        let enhanced = self.infer_oldest()?;
        let waveform = self.dsp.synthesize(&enhanced)?;
        output.copy_from_slice(&waveform);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StageError> {
        self.dsp.reset();
        self.encoder_state.fill(0.0);
        self.erb_state.fill(0.0);
        self.df_state.fill(0.0);
        self.spectra.clear();
        self.erb_features.clear();
        self.df_features.clear();
        Ok(())
    }
}

struct DeepFilterDsp {
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    analysis: Vec<f32>,
    synthesis: Vec<f32>,
    spectrum: Vec<Complex32>,
    erb_widths: Vec<usize>,
    erb_normalization: Vec<f32>,
    df_normalization: Vec<f32>,
}

impl DeepFilterDsp {
    fn new() -> Self {
        let mut planner = FftPlanner::new();
        Self {
            window: vorbis_window(),
            forward: planner.plan_fft_forward(FFT_SIZE),
            inverse: planner.plan_fft_inverse(FFT_SIZE),
            analysis: vec![0.0; FFT_SIZE],
            synthesis: vec![0.0; FRAME_SAMPLES],
            spectrum: vec![Complex32::new(0.0, 0.0); FFT_SIZE],
            erb_widths: erb_widths(),
            erb_normalization: linear_state(-60.0, -90.0, ERB_BANDS),
            df_normalization: linear_state(0.001, 0.0001, DF_BINS),
        }
    }

    fn analyze(&mut self, input: &[f32]) -> Vec<Complex32> {
        self.analysis.copy_within(FRAME_SAMPLES.., 0);
        self.analysis[FFT_SIZE - FRAME_SAMPLES..].copy_from_slice(input);
        for (index, value) in self.spectrum.iter_mut().enumerate() {
            *value = Complex32::new(self.analysis[index] * self.window[index], 0.0);
        }
        self.forward.process(&mut self.spectrum);
        let scale = reciprocal(FFT_SIZE);
        self.spectrum[..SPECTRUM_BINS]
            .iter()
            .map(|value| *value * scale)
            .collect()
    }

    fn synthesize(&mut self, values: &[f32]) -> Result<Vec<f32>, StageError> {
        if values.len() != SPECTRUM_BINS * 2 {
            return Err(backend_error(format!(
                "enhanced spectrum has {} values, expected {}",
                values.len(),
                SPECTRUM_BINS * 2
            )));
        }
        for index in 0..SPECTRUM_BINS {
            self.spectrum[index] = Complex32::new(values[index * 2], values[index * 2 + 1]);
        }
        for index in SPECTRUM_BINS..FFT_SIZE {
            self.spectrum[index] = self.spectrum[FFT_SIZE - index].conj();
        }
        self.inverse.process(&mut self.spectrum);
        let mut output = vec![0.0_f32; FRAME_SAMPLES];
        for (index, sample) in output.iter_mut().enumerate() {
            *sample = self.spectrum[index]
                .re
                .mul_add(self.window[index], self.synthesis[index]);
            self.synthesis[index] =
                self.spectrum[index + FRAME_SAMPLES].re * self.window[index + FRAME_SAMPLES];
        }
        Ok(output)
    }

    fn erb_features(&mut self, spectrum: &[Complex32]) -> Vec<f32> {
        let mut output = Vec::with_capacity(ERB_BANDS);
        let mut offset = 0;
        for (band, width) in self.erb_widths.iter().copied().enumerate() {
            let energy = spectrum[offset..offset + width]
                .iter()
                .map(Complex32::norm_sqr)
                .sum::<f32>()
                / usize_to_f32(width);
            let value = 10.0 * (energy + 1e-10).log10();
            let state = &mut self.erb_normalization[band];
            *state = value.mul_add(1.0 - NORMALIZATION_ALPHA, *state * NORMALIZATION_ALPHA);
            output.push((value - *state) / 40.0);
            offset += width;
        }
        output
    }

    fn df_features(&mut self, spectrum: &[Complex32]) -> Vec<f32> {
        let mut output = Vec::with_capacity(DF_BINS * 2);
        for (value, state) in spectrum[..DF_BINS].iter().zip(&mut self.df_normalization) {
            *state = value
                .norm()
                .mul_add(1.0 - NORMALIZATION_ALPHA, *state * NORMALIZATION_ALPHA);
            let normalized = *value / state.sqrt();
            output.extend([normalized.re, normalized.im]);
        }
        output
    }

    fn reset(&mut self) {
        self.analysis.fill(0.0);
        self.synthesis.fill(0.0);
        self.spectrum.fill(Complex32::new(0.0, 0.0));
        self.erb_normalization = linear_state(-60.0, -90.0, ERB_BANDS);
        self.df_normalization = linear_state(0.001, 0.0001, DF_BINS);
    }
}

fn output_values(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &'static str,
    expected: usize,
) -> Result<Vec<f32>, StageError> {
    let (shape, values) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(backend_error)?;
    if values.len() != expected {
        return Err(backend_error(format!(
            "{name} has shape {shape:?} and {} values, expected {expected}",
            values.len()
        )));
    }
    Ok(values.to_vec())
}

fn flatten_complex(values: &[Complex32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(values.len() * 2);
    for value in values {
        output.extend([value.re, value.im]);
    }
    output
}

fn linear_state(start: f32, end: f32, count: usize) -> Vec<f32> {
    let denominator = usize_to_f32(count - 1);
    (0..count)
        .map(|index| start + (end - start) * usize_to_f32(index) / denominator)
        .collect()
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "ERB bin calculations operate on fixed positive FFT dimensions and must match libDF's f32 rounding"
)]
fn erb_widths() -> Vec<usize> {
    let nyquist = SAMPLE_RATE as f32 / 2.0;
    let frequency_width = SAMPLE_RATE as f32 / FFT_SIZE as f32;
    let erb_high = 9.265 * (nyquist / (24.7 * 9.265)).ln_1p();
    let step = erb_high / ERB_BANDS as f32;
    let mut widths = vec![0; ERB_BANDS];
    let mut previous_frequency = 0_i32;
    let mut overflow = 0_i32;
    for band in 1..=ERB_BANDS {
        let erb = band as f32 * step;
        let frequency = 24.7 * 9.265 * (erb / 9.265).exp_m1();
        let frequency_bin = (frequency / frequency_width).round() as i32;
        let mut width = frequency_bin - previous_frequency - overflow;
        if width < MIN_ERB_BINS as i32 {
            overflow = MIN_ERB_BINS as i32 - width;
            width = MIN_ERB_BINS as i32;
        } else {
            overflow = 0;
        }
        widths[band - 1] = width as usize;
        previous_frequency = frequency_bin;
    }
    widths[ERB_BANDS - 1] += 1;
    let excess = widths.iter().sum::<usize>() - SPECTRUM_BINS;
    widths[ERB_BANDS - 1] -= excess;
    widths
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "the fixed 960-point Vorbis window is the f32 DeepFilterNet model contract"
)]
fn vorbis_window() -> Vec<f32> {
    (0..FFT_SIZE)
        .map(|index| {
            let inner = (std::f64::consts::PI * (index as f64 + 0.5) / FFT_SIZE as f64).sin();
            (std::f64::consts::FRAC_PI_2 * inner * inner).sin() as f32
        })
        .collect()
}

#[allow(
    clippy::cast_precision_loss,
    reason = "audio frame and band dimensions are far below f32's exact integer range"
)]
const fn usize_to_f32(value: usize) -> f32 {
    value as f32
}

fn reciprocal(value: usize) -> f32 {
    1.0 / usize_to_f32(value)
}

fn backend_error(error: impl std::fmt::Display) -> StageError {
    StageError::Backend {
        stage: DESCRIPTOR.id,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erb_filterbank_covers_every_bin() {
        let widths = erb_widths();
        assert_eq!(widths.len(), ERB_BANDS);
        assert_eq!(widths.iter().sum::<usize>(), SPECTRUM_BINS);
        assert!(widths.iter().all(|width| *width >= MIN_ERB_BINS));
    }

    #[test]
    fn silent_dsp_is_finite() -> Result<(), StageError> {
        let mut dsp = DeepFilterDsp::new();
        let spectrum = dsp.analyze(&[0.0; FRAME_SAMPLES]);
        let output = dsp.synthesize(&flatten_complex(&spectrum))?;
        assert!(output.iter().all(|sample| sample.is_finite()));
        Ok(())
    }
}
