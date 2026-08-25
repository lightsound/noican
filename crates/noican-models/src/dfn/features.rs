//! The `DeepFilterNet` feature front-end.
//!
//! These models do not take a raw spectrum. They take ERB band energies and a
//! low-frequency slice of the complex spectrum, both passed through a running
//! exponential normalisation whose starting values are not zero — a detail that
//! decides whether the first second of output is right.
//!
//! Ported from the reference implementation in
//! [`libDF`](https://github.com/Rikorose/DeepFilterNet) (MIT OR Apache-2.0) and
//! checked against its output; see `THIRD_PARTY_NOTICES.md`.

// clippy offers more accurate spellings of two expressions here — `exp_m1` for
// `exp() - 1`, and fused multiply-add for `a + b * c`. Both are declined on
// purpose. These formulas define which FFT bins fall in which ERB band, and the
// band edges are decided by a `round()` that a last-bit difference can flip. A
// one-bin shift means the trained weights see the wrong frequencies, so
// reproducing the reference's arithmetic matters more than improving on it.
#![expect(
    clippy::imprecise_flops,
    clippy::suboptimal_flops,
    reason = "these expressions reproduce the reference implementation's arithmetic exactly, which \
              is what keeps the ERB band edges identical to the ones the models were trained with"
)]

use noican_core::Spectrum;

/// Starting values of the ERB mean-normalisation history, in decibels.
///
/// A linear ramp between these two, low band to high band. Starting from zero
/// would make the model see a large spurious level in every band until the
/// running averages converge.
const MEAN_NORM_INIT: (f32, f32) = (-60.0, -90.0);

/// Starting values of the complex unit-normalisation history.
const UNIT_NORM_INIT: (f32, f32) = (0.001, 0.0001);

/// Divisor applied after mean normalisation, matching the reference.
const MEAN_NORM_SCALE: f32 = 40.0;

/// Floor added before taking the logarithm of a band energy.
const LOG_FLOOR: f32 = 1e-10;

/// Converts a frequency in hertz to the ERB scale.
fn frequency_to_erb(hertz: f32) -> f32 {
    9.265 * (hertz / (24.7 * 9.265)).ln_1p()
}

/// Converts a position on the ERB scale back to hertz.
fn erb_to_frequency(erb: f32) -> f32 {
    24.7 * 9.265 * ((erb / 9.265).exp() - 1.0)
}

/// Widths, in FFT bins, of each ERB band.
///
/// The bands are equal-width on the ERB scale, then widened where that would
/// leave fewer than `min_bins_per_band` bins, with the shortfall carried forward
/// so the widths still tile the spectrum exactly.
#[must_use]
pub fn erb_band_widths(
    sample_rate: u32,
    n_fft: usize,
    bands: usize,
    min_bins_per_band: usize,
) -> Vec<usize> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "audio sample rates and transform sizes are exact in f32"
    )]
    let (rate, fft_size) = (sample_rate as f32, n_fft as f32);
    let bin_width = rate / fft_size;
    let erb_low = frequency_to_erb(0.0);
    let erb_high = frequency_to_erb(rate / 2.0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "band counts are small integers, exact in f32"
    )]
    let step = (erb_high - erb_low) / bands as f32;

    let mut widths = vec![0usize; bands];
    let mut previous_bin = 0i64;
    let mut carried = 0i64;
    let min_bins = i64::try_from(min_bins_per_band).unwrap_or(i64::MAX);

    for (index, width) in widths.iter_mut().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "band index is bounded by the band count"
        )]
        let boundary = erb_to_frequency(erb_low + (index + 1) as f32 * step);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the boundary bin index cannot exceed n_fft / 2"
        )]
        let boundary_bin = (boundary / bin_width).round() as i64;

        let mut bins = boundary_bin - previous_bin - carried;
        if bins < min_bins {
            carried = min_bins - bins;
            bins = min_bins;
        } else {
            carried = 0;
        }
        *width = usize::try_from(bins).unwrap_or(0);
        previous_bin = boundary_bin;
    }

    // The one-sided spectrum has n_fft / 2 + 1 bins, one more than the loop
    // above accounts for; give it to the top band, then trim any overshoot.
    let bins = n_fft / 2 + 1;
    if let Some(last) = widths.last_mut() {
        *last += 1;
    }
    let total: usize = widths.iter().sum();
    if let Some(last) = widths.last_mut() {
        *last -= total.saturating_sub(bins).min(*last);
    }
    widths
}

/// The stateful feature front-end for one model.
#[derive(Debug)]
pub struct DfnFeatures {
    band_widths: Vec<usize>,
    /// Smoothing coefficient of both running averages.
    alpha: f32,
    mean_state: Vec<f32>,
    unit_state: Vec<f32>,
    mean_initial: Vec<f32>,
    unit_initial: Vec<f32>,
}

impl DfnFeatures {
    /// Builds the front-end for `config`.
    #[must_use]
    pub fn new(config: &super::DfnConfig) -> Self {
        let mean_initial = ramp(MEAN_NORM_INIT.0, MEAN_NORM_INIT.1, config.erb_bands);
        let unit_initial = ramp(UNIT_NORM_INIT.0, UNIT_NORM_INIT.1, config.df_bins);
        Self {
            band_widths: erb_band_widths(
                config.sample_rate,
                config.n_fft,
                config.erb_bands,
                config.min_bins_per_band,
            ),
            alpha: normalisation_alpha(config.sample_rate, config.hop, config.norm_tau),
            mean_state: mean_initial.clone(),
            unit_state: unit_initial.clone(),
            mean_initial,
            unit_initial,
        }
    }

    /// Widths, in FFT bins, of each ERB band.
    #[must_use]
    pub fn band_widths(&self) -> &[usize] {
        &self.band_widths
    }

    /// The smoothing coefficient in use.
    #[must_use]
    pub const fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Returns both running averages to their starting values.
    pub fn reset(&mut self) {
        self.mean_state.copy_from_slice(&self.mean_initial);
        self.unit_state.copy_from_slice(&self.unit_initial);
    }

    /// Writes the normalised ERB band feature for `frame` into `out`.
    ///
    /// `out.len()` must equal the band count.
    pub fn erb_feature(&mut self, frame: &Spectrum, out: &mut [f32]) {
        let mut bin = 0;
        for (&width, slot) in self.band_widths.iter().zip(out.iter_mut()) {
            let mut energy = 0.0f32;
            for offset in 0..width {
                energy += frame.power(bin + offset);
            }
            bin += width;
            #[expect(
                clippy::cast_precision_loss,
                reason = "band widths are small integers, exact in f32"
            )]
            let mean = energy / width.max(1) as f32;
            *slot = (mean + LOG_FLOOR).log10() * 10.0;
        }

        for (value, state) in out.iter_mut().zip(self.mean_state.iter_mut()) {
            *state = value.mul_add(1.0 - self.alpha, *state * self.alpha);
            *value = (*value - *state) / MEAN_NORM_SCALE;
        }
    }

    /// Writes the normalised complex feature for the low bins of `frame`.
    ///
    /// `real` and `imaginary` each receive `df_bins` values. They are separate
    /// because the encoder takes them as two channels, not as an interleaved
    /// trailing axis — the one place this differs from every other model in the
    /// catalog.
    pub fn spectral_feature(&mut self, frame: &Spectrum, real: &mut [f32], imaginary: &mut [f32]) {
        for (index, state) in self.unit_state.iter_mut().enumerate() {
            let value = frame.bin(index);
            let magnitude = value.norm();
            *state = magnitude.mul_add(1.0 - self.alpha, *state * self.alpha);
            let scale = state.sqrt();
            let inverse = if scale > 0.0 { 1.0 / scale } else { 0.0 };
            real[index] = value.re * inverse;
            imaginary[index] = value.im * inverse;
        }
    }
}

/// A linear ramp of `count` values from `start` to `end`.
fn ramp(start: f32, end: f32, count: usize) -> Vec<f32> {
    if count <= 1 {
        return vec![start; count];
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "feature dimensions are small integers, exact in f32"
    )]
    let step = (end - start) / (count - 1) as f32;
    (0..count)
        .map(|index| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "index is bounded by the feature dimension"
            )]
            let position = index as f32;
            start + position * step
        })
        .collect()
}

/// The smoothing coefficient for a given frame rate and time constant.
///
/// Reproduces the reference exactly, including its rounding loop: the published
/// value is truncated to the shortest decimal expansion that stays below one,
/// and the models were trained with that truncated value rather than with
/// `exp(-dt / tau)` itself.
fn normalisation_alpha(sample_rate: u32, hop: usize, tau: f32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "hop sizes and sample rates are exact in f32"
    )]
    let dt = hop as f32 / sample_rate as f32;
    let exact = (-dt / tau).exp();
    let mut rounded = 1.0f32;
    let mut precision = 3u32;
    while rounded >= 1.0 && precision < 10 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "10^precision with precision < 10 is exact in f32"
        )]
        let scale = 10i32.pow(precision) as f32;
        rounded = (exact * scale).round() / scale;
        precision += 1;
    }
    rounded
}

#[cfg(test)]
mod tests {
    use super::{DfnFeatures, erb_band_widths, normalisation_alpha, ramp};
    use crate::dfn::DfnConfig;
    use noican_core::{Complex32, Spectrum};

    fn config(sample_rate: u32, n_fft: usize, hop: usize, df_bins: usize) -> DfnConfig {
        DfnConfig {
            sample_rate,
            n_fft,
            hop,
            erb_bands: 32,
            df_bins,
            min_bins_per_band: 2,
            norm_tau: 1.0,
            df_order: 5,
            conv_lookahead: 0,
            df_lookahead: 0,
        }
    }

    #[test]
    fn band_widths_tile_the_spectrum_exactly() {
        for (rate, n_fft) in [(48_000u32, 960usize), (16_000, 320), (16_000, 512)] {
            let widths = erb_band_widths(rate, n_fft, 32, 2);
            assert_eq!(widths.len(), 32);
            assert_eq!(
                widths.iter().sum::<usize>(),
                n_fft / 2 + 1,
                "widths for {rate} Hz / {n_fft} do not tile the spectrum: {widths:?}"
            );
            assert!(
                widths.iter().all(|&width| width >= 2),
                "a band fell below the minimum: {widths:?}"
            );
        }
    }

    #[test]
    fn band_widths_grow_towards_high_frequencies() {
        let widths = erb_band_widths(48_000, 960, 32, 2);
        assert!(widths[0] < widths[31]);
        assert!(widths[31] > 40, "top band is only {} bins", widths[31]);
    }

    /// The reference rounds `exp(-dt / tau)` to three decimals at these
    /// settings, giving 0.99 rather than 0.990049...
    #[test]
    fn alpha_matches_the_reference_rounding() {
        assert!((normalisation_alpha(48_000, 480, 1.0) - 0.99).abs() < 1e-7);
        assert!((normalisation_alpha(16_000, 160, 1.0) - 0.99).abs() < 1e-7);
    }

    #[test]
    fn ramp_spans_its_endpoints() {
        let values = ramp(-60.0, -90.0, 32);
        assert!((values[0] + 60.0).abs() < 1e-5);
        assert!((values[31] + 90.0).abs() < 1e-5);
        assert_eq!(ramp(1.0, 2.0, 1), vec![1.0]);
        assert!(ramp(1.0, 2.0, 0).is_empty());
    }

    #[test]
    fn silence_produces_a_negative_feature_everywhere() {
        let mut features = DfnFeatures::new(&config(48_000, 960, 480, 96));
        let frame = Spectrum::zeroed(481);
        let mut erb = vec![0.0; 32];
        features.erb_feature(&frame, &mut erb);
        // 10 * log10(1e-10) is -100 dB, below the -60..-90 dB seed.
        assert!(
            erb.iter().all(|&value| value < 0.0),
            "silence produced a positive feature: {erb:?}"
        );
    }

    #[test]
    fn features_are_finite_and_reset_restores_them() {
        let mut features = DfnFeatures::new(&config(16_000, 320, 160, 64));
        assert_eq!(features.band_widths().len(), 32);
        assert!((features.alpha() - 0.99).abs() < 1e-7);

        let mut frame = Spectrum::zeroed(161);
        for index in 0..frame.len() {
            frame.set_bin(index, Complex32::new(0.5, -0.25));
        }

        let mut erb = vec![0.0; 32];
        let mut real = vec![0.0; 64];
        let mut imaginary = vec![0.0; 64];
        features.erb_feature(&frame, &mut erb);
        features.spectral_feature(&frame, &mut real, &mut imaginary);

        assert!(erb.iter().all(|value| value.is_finite()));
        assert!(real.iter().chain(&imaginary).all(|value| value.is_finite()));
        assert!(real.iter().any(|&value| value.abs() > 1e-6));
        // The two channels carry the real and imaginary parts separately, and
        // the input had a non-zero imaginary part.
        assert!(imaginary.iter().any(|&value| value.abs() > 1e-6));

        let first_pass = erb.clone();
        features.reset();
        features.erb_feature(&frame, &mut erb);
        for (after, before) in erb.iter().zip(&first_pass) {
            assert!(
                (after - before).abs() < 1e-9,
                "reset did not restore the running averages"
            );
        }
    }
}
