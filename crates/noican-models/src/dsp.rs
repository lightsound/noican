//! Small DSP helpers shared by the STFT-outside-the-graph stages.

use noican_core::StageError;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Vorbis window: `w[i] = sin(pi/2 * sin^2(pi/2 * (i + 0.5) / (N/2)))`.
///
/// Satisfies squared-COLA at 50% overlap, so analysis + synthesis
/// windowing needs no extra normalization (DPDFNet, DeepFilterNet family).
#[must_use]
pub fn vorbis_window(len: usize) -> Vec<f32> {
    let half = f64::from(u32::try_from(len).unwrap_or(u32::MAX)) / 2.0;
    (0..len)
        .map(|i| {
            let x = f64::from(u32::try_from(i).unwrap_or(u32::MAX));
            let s = (0.5 * std::f64::consts::PI * (x + 0.5) / half).sin();
            #[allow(
                clippy::cast_possible_truncation,
                reason = "window values are in [0, 1]"
            )]
            let w = (0.5 * std::f64::consts::PI * s * s).sin() as f32;
            w
        })
        .collect()
}

/// Periodic Hann window (`torch.hann_window` semantics):
/// `w[n] = 0.5 * (1 - cos(2*pi*n / N))` — divisor `N`, not `N - 1`.
#[must_use]
pub fn periodic_hann_window(len: usize) -> Vec<f32> {
    let n = f64::from(u32::try_from(len).unwrap_or(u32::MAX));
    (0..len)
        .map(|i| {
            let x = f64::from(u32::try_from(i).unwrap_or(u32::MAX));
            #[allow(
                clippy::cast_possible_truncation,
                reason = "window values are in [0, 1]"
            )]
            let w = (0.5 * (1.0 - (2.0 * std::f64::consts::PI * x / n).cos())) as f32;
            w
        })
        .collect()
}

/// Square root of the periodic Hann window (`hann_sqrt` in DPDFNet
/// metadata for some profiles).
#[must_use]
pub fn sqrt_hann_window(len: usize) -> Vec<f32> {
    periodic_hann_window(len).iter().map(|w| w.sqrt()).collect()
}

/// A paired forward/inverse real FFT of a fixed size with numpy
/// `rfft`/`irfft` scaling: forward is unnormalized, inverse divides by `n`.
pub struct FftPair {
    n: usize,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    time_buf: Vec<f32>,
    freq_buf: Vec<Complex32>,
    scratch_fwd: Vec<Complex32>,
    scratch_inv: Vec<Complex32>,
}

impl std::fmt::Debug for FftPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FftPair")
            .field("n", &self.n)
            .finish_non_exhaustive()
    }
}

impl FftPair {
    /// Plans forward and inverse transforms of size `n` (must be even).
    #[must_use]
    pub fn new(n: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let scratch_fwd = forward.make_scratch_vec();
        let scratch_inv = inverse.make_scratch_vec();
        Self {
            n,
            time_buf: vec![0.0; n],
            freq_buf: vec![Complex32::new(0.0, 0.0); n / 2 + 1],
            forward,
            inverse,
            scratch_fwd,
            scratch_inv,
        }
    }

    /// Number of complex bins (`n / 2 + 1`).
    #[must_use]
    pub const fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Forward transform of `frame` (length `n`), writing interleaved
    /// `[re0, im0, re1, im1, ...]` into `spec` (length `2 * bins`).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on an internal FFT failure.
    pub fn forward_interleaved(
        &mut self,
        frame: &[f32],
        spec: &mut [f32],
    ) -> Result<(), StageError> {
        self.time_buf.copy_from_slice(frame);
        self.forward
            .process_with_scratch(
                &mut self.time_buf,
                &mut self.freq_buf,
                &mut self.scratch_fwd,
            )
            .map_err(|e| StageError::Inference(format!("rfft failed: {e}")))?;
        for (i, c) in self.freq_buf.iter().enumerate() {
            spec[2 * i] = c.re;
            spec[2 * i + 1] = c.im;
        }
        Ok(())
    }

    /// Inverse transform of interleaved `spec` (length `2 * bins`) into
    /// `frame` (length `n`), scaled by `1 / n` (numpy `irfft` semantics).
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Inference`] on an internal FFT failure.
    pub fn inverse_interleaved(
        &mut self,
        spec: &[f32],
        frame: &mut [f32],
    ) -> Result<(), StageError> {
        for (i, c) in self.freq_buf.iter_mut().enumerate() {
            c.re = spec[2 * i];
            c.im = spec[2 * i + 1];
        }
        // realfft requires the Nyquist/DC imaginary parts to be zero.
        self.freq_buf[0].im = 0.0;
        let last = self.freq_buf.len() - 1;
        self.freq_buf[last].im = 0.0;
        self.inverse
            .process_with_scratch(
                &mut self.freq_buf,
                &mut self.time_buf,
                &mut self.scratch_inv,
            )
            .map_err(|e| StageError::Inference(format!("irfft failed: {e}")))?;
        #[allow(
            clippy::cast_precision_loss,
            reason = "FFT sizes are tiny; exact f32 representation"
        )]
        let scale = 1.0 / self.n as f32;
        for (out, v) in frame.iter_mut().zip(&self.time_buf) {
            *out = v * scale;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vorbis_window_satisfies_squared_cola_at_half_overlap() {
        let n = 960;
        let w = vorbis_window(n);
        for i in 0..n / 2 {
            let s = w[i].mul_add(w[i], w[i + n / 2] * w[i + n / 2]);
            assert!((s - 1.0).abs() < 1e-5, "at {i}: {s}");
        }
    }

    #[test]
    fn fft_round_trip_is_identity() {
        let n = 512;
        let mut pair = FftPair::new(n);
        let frame: Vec<f32> = (0..n)
            .map(|i| {
                #[allow(clippy::cast_precision_loss, reason = "test indices are small")]
                let x = i as f32 * 0.1;
                x.sin()
            })
            .collect();
        let mut spec = vec![0.0; 2 * pair.bins()];
        let mut back = vec![0.0; n];
        pair.forward_interleaved(&frame, &mut spec).expect("fwd");
        pair.inverse_interleaved(&spec, &mut back).expect("inv");
        for (a, b) in frame.iter().zip(&back) {
            assert!((a - b).abs() < 1e-5);
        }
    }
}
