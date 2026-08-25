//! Streaming short-time Fourier transform and its inverse.
//!
//! Most published enhancement models take a spectrum frame rather than a
//! waveform, so the analysis and synthesis transforms live outside the ONNX
//! graph and have to match what the model was trained with exactly: the same
//! window, the same hop, and no normalisation.
//!
//! Both halves are streaming and allocation-free after construction. The
//! synthesis path uses weighted overlap-add with the steady-state
//! sum-of-squares envelope, which is exact once `n_fft` samples have been
//! emitted.

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub use realfft::num_complex::Complex32;

use crate::error::{Error, Result};
use crate::window::WindowKind;

/// One frame of a one-sided spectrum, stored as interleaved `[real, imag]`
/// pairs.
///
/// This is the layout every ONNX graph in the project expects for its spectrum
/// input (`[batch, time, freq, 2]` or `[batch, freq, time, 2]`), so frames can
/// be handed to the inference session without a repacking step.
#[derive(Debug, Clone)]
pub struct Spectrum {
    bins: Box<[f32]>,
}

impl Spectrum {
    /// Creates a zeroed spectrum with `bins` frequency bins.
    #[must_use]
    pub fn zeroed(bins: usize) -> Self {
        Self {
            bins: vec![0.0; bins * 2].into_boxed_slice(),
        }
    }

    /// Number of frequency bins, i.e. `n_fft / 2 + 1`.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.bins.len() / 2
    }

    /// Whether the spectrum has no bins.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bins.is_empty()
    }

    /// The interleaved `[real, imag]` samples, for handing to an inference
    /// session.
    #[must_use]
    pub const fn as_interleaved(&self) -> &[f32] {
        &self.bins
    }

    /// The interleaved samples, mutably, for receiving inference output.
    #[must_use]
    pub const fn as_interleaved_mut(&mut self) -> &mut [f32] {
        &mut self.bins
    }

    /// The complex value of bin `index`.
    #[must_use]
    pub fn bin(&self, index: usize) -> Complex32 {
        Complex32::new(self.bins[index * 2], self.bins[index * 2 + 1])
    }

    /// Overwrites bin `index`.
    pub const fn set_bin(&mut self, index: usize, value: Complex32) {
        self.bins[index * 2] = value.re;
        self.bins[index * 2 + 1] = value.im;
    }

    /// Squared magnitude of bin `index`.
    #[must_use]
    pub fn power(&self, index: usize) -> f32 {
        let re = self.bins[index * 2];
        let im = self.bins[index * 2 + 1];
        re.mul_add(re, im * im)
    }

    /// Copies the contents of `other`.
    ///
    /// # Panics
    ///
    /// Panics if the two spectra have different bin counts.
    pub fn copy_from(&mut self, other: &Self) {
        self.bins.copy_from_slice(&other.bins);
    }
}

/// Shared configuration of an analysis/synthesis pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StftConfig {
    /// Transform size in samples.
    pub n_fft: usize,
    /// Advance between consecutive frames, in samples. Must divide `n_fft`.
    pub hop: usize,
    /// Window applied on analysis and, squared-normalised, on synthesis.
    pub window: WindowKind,
}

impl StftConfig {
    /// Number of one-sided frequency bins produced by this configuration.
    #[must_use]
    pub const fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    fn validate(&self) -> Result<()> {
        if self.n_fft == 0 || self.hop == 0 {
            return Err(Error::InvalidConfiguration(
                "n_fft and hop must be non-zero".to_owned(),
            ));
        }
        if !self.n_fft.is_multiple_of(2) {
            return Err(Error::InvalidConfiguration(format!(
                "n_fft must be even (got {})",
                self.n_fft
            )));
        }
        if !self.n_fft.is_multiple_of(self.hop) {
            return Err(Error::InvalidConfiguration(format!(
                "hop ({}) must divide n_fft ({})",
                self.hop, self.n_fft
            )));
        }
        Ok(())
    }
}

/// Turns a stream of `hop`-sized blocks into spectrum frames.
pub struct StftAnalyzer {
    config: StftConfig,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Box<[f32]>,
    /// Sliding window of the most recent `n_fft` input samples, oldest first.
    history: Box<[f32]>,
    time_domain: Box<[f32]>,
    frequency_domain: Box<[Complex32]>,
    scratch: Box<[Complex32]>,
}

// The FFT plans behind `Arc<dyn RealToComplex>` are not `Debug`; the
// configuration is the only part worth printing anyway.
impl std::fmt::Debug for StftAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StftAnalyzer")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl StftAnalyzer {
    /// Builds an analyzer for `config`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfiguration`] if the configuration is not a
    /// valid transform (see [`StftConfig`]).
    pub fn new(config: StftConfig) -> Result<Self> {
        config.validate()?;
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(config.n_fft);
        let scratch = fft.make_scratch_vec().into_boxed_slice();
        Ok(Self {
            window: config.window.build(config.n_fft).into_boxed_slice(),
            history: vec![0.0; config.n_fft].into_boxed_slice(),
            time_domain: vec![0.0; config.n_fft].into_boxed_slice(),
            frequency_domain: vec![Complex32::default(); config.bins()].into_boxed_slice(),
            scratch,
            fft,
            config,
        })
    }

    /// The configuration this analyzer was built with.
    #[must_use]
    pub const fn config(&self) -> StftConfig {
        self.config
    }

    /// Clears the sliding window.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
    }

    /// Consumes one hop of samples and writes the resulting frame to `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BufferLength`] if `input` is not exactly one hop, or if
    /// `out` has the wrong number of bins.
    pub fn process(&mut self, input: &[f32], out: &mut Spectrum) -> Result<()> {
        if input.len() != self.config.hop {
            return Err(Error::BufferLength {
                expected: self.config.hop,
                actual: input.len(),
            });
        }
        if out.len() != self.config.bins() {
            return Err(Error::BufferLength {
                expected: self.config.bins(),
                actual: out.len(),
            });
        }

        self.history.rotate_left(self.config.hop);
        let tail = self.config.n_fft - self.config.hop;
        self.history[tail..].copy_from_slice(input);

        for (dst, (&sample, &weight)) in self
            .time_domain
            .iter_mut()
            .zip(self.history.iter().zip(self.window.iter()))
        {
            *dst = sample * weight;
        }

        self.fft
            .process_with_scratch(
                &mut self.time_domain,
                &mut self.frequency_domain,
                &mut self.scratch,
            )
            .map_err(|error| Error::Stage(format!("forward FFT failed: {error}")))?;

        for (index, &value) in self.frequency_domain.iter().enumerate() {
            out.set_bin(index, value);
        }
        Ok(())
    }
}

/// Turns a stream of spectrum frames back into `hop`-sized blocks.
pub struct StftSynthesizer {
    config: StftConfig,
    fft: Arc<dyn ComplexToReal<f32>>,
    window: Box<[f32]>,
    /// Steady-state sum of squared window values at each position within a hop.
    normalisation: Box<[f32]>,
    /// Overlap-add accumulator, oldest sample first.
    accumulator: Box<[f32]>,
    time_domain: Box<[f32]>,
    frequency_domain: Box<[Complex32]>,
    scratch: Box<[Complex32]>,
}

// See the note on `StftAnalyzer`'s `Debug` implementation.
impl std::fmt::Debug for StftSynthesizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StftSynthesizer")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl StftSynthesizer {
    /// Builds a synthesizer for `config`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfiguration`] if the configuration is not a
    /// valid transform (see [`StftConfig`]).
    pub fn new(config: StftConfig) -> Result<Self> {
        config.validate()?;
        let fft = RealFftPlanner::<f32>::new().plan_fft_inverse(config.n_fft);
        let scratch = fft.make_scratch_vec().into_boxed_slice();
        let window = config.window.build(config.n_fft);

        // With `hop` dividing `n_fft`, exactly `n_fft / hop` frames overlap any
        // given sample once the pipeline is primed, and the envelope repeats
        // every hop.
        let overlap = config.n_fft / config.hop;
        let normalisation: Vec<f32> = (0..config.hop)
            .map(|offset| {
                (0..overlap)
                    .map(|frame| window[offset + frame * config.hop].powi(2))
                    .sum()
            })
            .collect();

        Ok(Self {
            window: window.into_boxed_slice(),
            normalisation: normalisation.into_boxed_slice(),
            accumulator: vec![0.0; config.n_fft].into_boxed_slice(),
            time_domain: vec![0.0; config.n_fft].into_boxed_slice(),
            frequency_domain: vec![Complex32::default(); config.bins()].into_boxed_slice(),
            scratch,
            fft,
            config,
        })
    }

    /// The configuration this synthesizer was built with.
    #[must_use]
    pub const fn config(&self) -> StftConfig {
        self.config
    }

    /// Clears the overlap-add accumulator.
    pub fn reset(&mut self) {
        self.accumulator.fill(0.0);
    }

    /// Consumes one frame and writes one hop of samples to `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BufferLength`] if `frame` has the wrong number of bins
    /// or `out` is not exactly one hop.
    pub fn process(&mut self, frame: &Spectrum, out: &mut [f32]) -> Result<()> {
        if frame.len() != self.config.bins() {
            return Err(Error::BufferLength {
                expected: self.config.bins(),
                actual: frame.len(),
            });
        }
        if out.len() != self.config.hop {
            return Err(Error::BufferLength {
                expected: self.config.hop,
                actual: out.len(),
            });
        }

        for index in 0..self.config.bins() {
            self.frequency_domain[index] = frame.bin(index);
        }
        // A real inverse transform requires purely real DC and Nyquist bins;
        // models occasionally emit a residual imaginary part there.
        self.frequency_domain[0].im = 0.0;
        if let Some(last) = self.frequency_domain.last_mut() {
            last.im = 0.0;
        }

        self.fft
            .process_with_scratch(
                &mut self.frequency_domain,
                &mut self.time_domain,
                &mut self.scratch,
            )
            .map_err(|error| Error::Stage(format!("inverse FFT failed: {error}")))?;

        #[expect(
            clippy::cast_precision_loss,
            reason = "transform sizes are small powers of two, exact in f32"
        )]
        let inverse_scale = 1.0 / self.config.n_fft as f32;
        for (accumulated, (&sample, &weight)) in self
            .accumulator
            .iter_mut()
            .zip(self.time_domain.iter().zip(self.window.iter()))
        {
            *accumulated = (sample * inverse_scale).mul_add(weight, *accumulated);
        }

        for (index, slot) in out.iter_mut().enumerate() {
            let denominator = self.normalisation[index];
            *slot = if denominator > 1e-8 {
                self.accumulator[index] / denominator
            } else {
                0.0
            };
        }

        self.accumulator.rotate_left(self.config.hop);
        let tail = self.config.n_fft - self.config.hop;
        self.accumulator[tail..].fill(0.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Spectrum, StftAnalyzer, StftConfig, StftSynthesizer};
    use crate::window::WindowKind;

    fn config(window: WindowKind) -> StftConfig {
        StftConfig {
            n_fft: 960,
            hop: 480,
            window,
        }
    }

    fn round_trip(window: WindowKind, input: &[f32]) -> Vec<f32> {
        let config = config(window);
        let mut analyzer = StftAnalyzer::new(config).unwrap();
        let mut synthesizer = StftSynthesizer::new(config).unwrap();
        let mut frame = Spectrum::zeroed(config.bins());
        let mut output = Vec::with_capacity(input.len());
        let mut hop = vec![0.0; config.hop];

        for block in input.chunks_exact(config.hop) {
            analyzer.process(block, &mut frame).unwrap();
            synthesizer.process(&frame, &mut hop).unwrap();
            output.extend_from_slice(&hop);
        }
        output
    }

    fn sine(rate: f32, freq: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "test fixture")]
                let t = i as f32 / rate;
                0.5 * (2.0 * core::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    /// Analysis followed by synthesis reproduces the input delayed by one hop.
    #[test]
    fn round_trip_is_transparent_for_every_window() {
        for window in [WindowKind::Vorbis, WindowKind::Hann, WindowKind::HannSqrt] {
            let input = sine(48_000.0, 440.0, 9_600);
            let output = round_trip(window, &input);
            let delay = 480;
            // Skip the first two frames while the overlap-add primes.
            let start = 960;
            let compared = 4_800;
            for i in start..start + compared {
                let expected = input[i - delay];
                let actual = output[i];
                assert!(
                    (expected - actual).abs() < 1e-4,
                    "{window:?} sample {i}: expected {expected}, got {actual}"
                );
            }
        }
    }

    /// Pinned against `numpy.fft.rfft` of the same windowed frame.
    ///
    /// Every spectral model in the catalog was trained against a `NumPy` or
    /// `PyTorch` transform, so agreeing with one to six decimals is the
    /// property that makes those models usable at all — and a polarity or
    /// scaling error here is inaudible on its own and only surfaces when
    /// comparing output against another implementation.
    #[test]
    fn analysis_matches_numpy_rfft() {
        let config = StftConfig {
            n_fft: 512,
            hop: 256,
            window: WindowKind::Hann,
        };
        let mut analyzer = StftAnalyzer::new(config).unwrap();
        let mut frame = Spectrum::zeroed(config.bins());

        // Same deterministic sequence the reference values were generated from:
        // frac(sin(i * 12.9898) * 43758.5453) mapped to [-1, 1).
        let input: Vec<f32> = (0..config.hop * 5)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "test fixture")]
                let x = (i as f64 * 12.9898).sin() * 43758.5453;
                #[expect(clippy::cast_possible_truncation, reason = "test fixture")]
                let fraction = (x - x.floor()) as f32;
                fraction.mul_add(2.0, -1.0)
            })
            .collect();
        for block in input.chunks_exact(config.hop) {
            analyzer.process(block, &mut frame).unwrap();
        }

        let expected = [
            (0.676_762, 0.0),
            (-1.845_93, 8.304_72),
            (-4.531_114, -7.977_563),
            (13.242_738, 9.550_798),
            (-9.170_159, -12.430_621),
            (-1.742_136, 5.288_396),
        ];
        for (index, (re, im)) in expected.into_iter().enumerate() {
            let actual = frame.bin(index);
            assert!(
                (actual.re - re).abs() < 1e-4 && (actual.im - im).abs() < 1e-4,
                "bin {index}: expected {re}+{im}i, got {}+{}i",
                actual.re,
                actual.im
            );
        }
    }

    #[test]
    fn a_tone_lands_in_its_own_bin() {
        let config = config(WindowKind::Vorbis);
        // Exactly on bin 50, so the tone is periodic in the analysis window.
        let bin = 50;
        #[expect(clippy::cast_precision_loss, reason = "test fixture")]
        let freq = 48_000.0 * bin as f32 / config.n_fft as f32;
        let input = sine(48_000.0, freq, config.hop * 6);

        let mut analyzer = StftAnalyzer::new(config).unwrap();
        let mut frame = Spectrum::zeroed(config.bins());
        for block in input.chunks_exact(config.hop) {
            analyzer.process(block, &mut frame).unwrap();
        }

        assert_eq!(frame.len(), 481);
        assert!(!frame.is_empty());

        let peak = (0..frame.len())
            .max_by(|a, b| frame.power(*a).total_cmp(&frame.power(*b)))
            .unwrap();
        assert_eq!(peak, bin);

        // The Vorbis window's main lobe spans a couple of bins; beyond it the
        // energy has to collapse.
        let peak_power = frame.power(bin);
        for index in 0..frame.len() {
            if index.abs_diff(bin) > 3 {
                assert!(
                    frame.power(index) < peak_power * 1e-4,
                    "bin {index} holds {} against a peak of {peak_power}",
                    frame.power(index)
                );
            }
        }
    }

    #[test]
    fn rejects_invalid_configurations() {
        let bad_hop = StftConfig {
            n_fft: 960,
            hop: 7,
            window: WindowKind::Vorbis,
        };
        assert!(StftAnalyzer::new(bad_hop).is_err());
        assert!(StftSynthesizer::new(bad_hop).is_err());

        let odd = StftConfig {
            n_fft: 15,
            hop: 5,
            window: WindowKind::Hann,
        };
        assert!(StftAnalyzer::new(odd).is_err());

        let zero = StftConfig {
            n_fft: 0,
            hop: 0,
            window: WindowKind::Hann,
        };
        assert!(StftAnalyzer::new(zero).is_err());
    }

    #[test]
    fn rejects_mismatched_buffers() {
        let config = config(WindowKind::Vorbis);
        let mut analyzer = StftAnalyzer::new(config).unwrap();
        let mut synthesizer = StftSynthesizer::new(config).unwrap();
        let mut frame = Spectrum::zeroed(config.bins());

        assert!(analyzer.process(&[0.0; 10], &mut frame).is_err());
        let mut wrong_bins = Spectrum::zeroed(3);
        assert!(analyzer.process(&vec![0.0; 480], &mut wrong_bins).is_err());
        assert!(synthesizer.process(&wrong_bins, &mut [0.0; 480]).is_err());
        assert!(synthesizer.process(&frame, &mut [0.0; 7]).is_err());

        frame.copy_from(&Spectrum::zeroed(config.bins()));
        assert_eq!(analyzer.config(), config);
        assert_eq!(synthesizer.config(), config);
        analyzer.reset();
        synthesizer.reset();
    }
}
