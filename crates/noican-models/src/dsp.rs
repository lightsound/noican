//! Fixed-hop STFT and overlap-add synthesis used around spectral ONNX graphs.

use std::sync::Arc;

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use thiserror::Error;

/// Analysis/synthesis window expected by a spectral model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Window {
    /// DeepFilterNet-family Vorbis window.
    Vorbis,
    /// Periodic Hann window.
    Hann,
}

/// Stateful fixed-hop spectral transform.
pub struct StreamingStft {
    fft_size: usize,
    hop_size: usize,
    bins: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    analysis: Vec<f32>,
    synthesis: Vec<f32>,
    normalization: Vec<f32>,
    spectrum: Vec<Complex32>,
}

impl StreamingStft {
    /// Build a transform with an overlap-add-compatible window.
    ///
    /// # Errors
    ///
    /// Returns [`DspError::InvalidGeometry`] unless the hop evenly divides
    /// the FFT size and is no larger than it.
    pub fn new(fft_size: usize, hop_size: usize, window: Window) -> Result<Self, DspError> {
        if fft_size == 0
            || hop_size == 0
            || hop_size > fft_size
            || !fft_size.is_multiple_of(hop_size)
        {
            return Err(DspError::InvalidGeometry { fft_size, hop_size });
        }
        let mut planner = FftPlanner::new();
        Ok(Self {
            fft_size,
            hop_size,
            bins: fft_size / 2 + 1,
            window: make_window(fft_size, window),
            forward: planner.plan_fft_forward(fft_size),
            inverse: planner.plan_fft_inverse(fft_size),
            analysis: vec![0.0; fft_size],
            synthesis: vec![0.0; fft_size],
            normalization: vec![0.0; fft_size],
            spectrum: vec![Complex32::new(0.0, 0.0); fft_size],
        })
    }

    /// Number of one-sided complex bins.
    #[must_use]
    pub const fn bins(&self) -> usize {
        self.bins
    }

    /// Convert one hop into interleaved real/imaginary one-sided bins.
    ///
    /// # Errors
    ///
    /// Returns [`DspError::InvalidHop`] for a non-matching input.
    pub fn analyze(&mut self, input: &[f32]) -> Result<Vec<f32>, DspError> {
        if input.len() != self.hop_size {
            return Err(DspError::InvalidHop {
                expected: self.hop_size,
                actual: input.len(),
            });
        }
        self.analysis.copy_within(self.hop_size.., 0);
        self.analysis[self.fft_size - self.hop_size..].copy_from_slice(input);
        for (index, bin) in self.spectrum.iter_mut().enumerate() {
            *bin = Complex32::new(self.analysis[index] * self.window[index], 0.0);
        }
        self.forward.process(&mut self.spectrum);
        let mut output = vec![0.0_f32; self.bins * 2];
        for (index, bin) in self.spectrum[..self.bins].iter().enumerate() {
            output[index * 2] = bin.re;
            output[index * 2 + 1] = bin.im;
        }
        Ok(output)
    }

    /// Convert interleaved real/imaginary one-sided bins into one audio hop.
    ///
    /// # Errors
    ///
    /// Returns [`DspError::InvalidSpectrum`] for a non-matching input.
    pub fn synthesize(&mut self, spectrum: &[f32]) -> Result<Vec<f32>, DspError> {
        let expected = self.bins * 2;
        if spectrum.len() != expected {
            return Err(DspError::InvalidSpectrum {
                expected,
                actual: spectrum.len(),
            });
        }
        for index in 0..self.bins {
            self.spectrum[index] = Complex32::new(spectrum[index * 2], spectrum[index * 2 + 1]);
        }
        for index in self.bins..self.fft_size {
            self.spectrum[index] = self.spectrum[self.fft_size - index].conj();
        }
        self.inverse.process(&mut self.spectrum);

        self.synthesis.copy_within(self.hop_size.., 0);
        self.synthesis[self.fft_size - self.hop_size..].fill(0.0);
        self.normalization.copy_within(self.hop_size.., 0);
        self.normalization[self.fft_size - self.hop_size..].fill(0.0);
        let inverse_scale = reciprocal(self.fft_size);
        for index in 0..self.fft_size {
            let window = self.window[index];
            self.synthesis[index] += self.spectrum[index].re * inverse_scale * window;
            self.normalization[index] += window * window;
        }
        let output = self
            .synthesis
            .iter()
            .zip(&self.normalization)
            .take(self.hop_size)
            .map(|(sample, weight)| {
                if *weight > f32::EPSILON {
                    sample / weight
                } else {
                    0.0
                }
            })
            .collect();
        Ok(output)
    }

    /// Clear transform and overlap-add history.
    pub fn reset(&mut self) {
        self.analysis.fill(0.0);
        self.synthesis.fill(0.0);
        self.normalization.fill(0.0);
        self.spectrum.fill(Complex32::new(0.0, 0.0));
    }
}

/// Spectral transform failures.
#[derive(Debug, Error)]
pub enum DspError {
    /// FFT and hop sizes cannot form a fixed overlap.
    #[error("invalid STFT geometry: fft_size={fft_size}, hop_size={hop_size}")]
    InvalidGeometry {
        /// FFT size.
        fft_size: usize,
        /// Hop size.
        hop_size: usize,
    },
    /// Analysis received the wrong number of samples.
    #[error("STFT expected {expected} input samples, received {actual}")]
    InvalidHop {
        /// Required hop size.
        expected: usize,
        /// Supplied sample count.
        actual: usize,
    },
    /// Synthesis received the wrong number of scalars.
    #[error("ISTFT expected {expected} spectral scalars, received {actual}")]
    InvalidSpectrum {
        /// Required scalar count.
        expected: usize,
        /// Supplied scalar count.
        actual: usize,
    },
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "window indices and FFT sizes are bounded audio dimensions; f32 coefficients are the model contract"
)]
fn make_window(size: usize, window: Window) -> Vec<f32> {
    match window {
        Window::Vorbis => (0..size)
            .map(|index| {
                let phase = std::f64::consts::PI * (index as f64 + 0.5) / size as f64;
                (std::f64::consts::FRAC_PI_2 * phase.sin().powi(2)).sin() as f32
            })
            .collect(),
        Window::Hann => (0..size)
            .map(|index| {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / size as f64).cos()) as f32
            })
            .collect(),
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "FFT sizes are below 2^24 and therefore exactly representable as f32"
)]
fn reciprocal(value: usize) -> f32 {
    1.0 / value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_geometry_is_rejected() {
        assert!(matches!(
            StreamingStft::new(512, 300, Window::Hann),
            Err(DspError::InvalidGeometry { .. })
        ));
    }

    #[test]
    fn silent_hops_remain_finite() -> Result<(), DspError> {
        let mut stft = StreamingStft::new(512, 256, Window::Hann)?;
        for _ in 0..4 {
            let spectrum = stft.analyze(&[0.0; 256])?;
            let output = stft.synthesize(&spectrum)?;
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(output.iter().all(|sample| sample.abs() <= f32::EPSILON));
        }
        Ok(())
    }
}
