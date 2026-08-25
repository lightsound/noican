//! Applying a `DeepFilterNet` model's two outputs to a spectrum.
//!
//! The graphs produce an ERB-band gain vector and a set of complex filter
//! coefficients; turning those into an enhanced spectrum happens here, because
//! the published exports leave it outside the graph.
//!
//! Both conventions below were pinned by measurement rather than read from
//! documentation, because the documentation does not state them. Guessing the
//! coefficient layout wrong costs 6 dB and guessing the tap order wrong costs
//! 17 dB, so the tests assert the shapes that make those mistakes fail.

use noican_core::{Complex32, Spectrum};

/// Multiplies each ERB band's bins by that band's gain.
///
/// The mask is one gain per band; `band_widths` says how many FFT bins each
/// band covers, and they tile the spectrum exactly.
pub fn apply_band_gains(spectrum: &mut Spectrum, gains: &[f32], band_widths: &[usize]) {
    let mut bin = 0;
    for (&width, &gain) in band_widths.iter().zip(gains) {
        for offset in 0..width {
            let index = bin + offset;
            if index >= spectrum.len() {
                return;
            }
            spectrum.set_bin(index, spectrum.bin(index) * gain);
        }
        bin += width;
    }
}

/// Applies the deep filter to the low `df_bins` bins.
///
/// `history` holds the noisy spectra, oldest first. `coefficients` is laid out
/// `[bin][tap][re, im]` — complex on the **last** axis — and the *last* tap
/// pairs with the newest frame.
///
/// A history shorter than `df_order` is treated as though the missing older
/// frames were silence, which is what the reference's zero-initialised buffer
/// represents. A longer history contributes only its newest `df_order` frames.
///
/// The result replaces the low bins of `out` outright rather than scaling them;
/// bins above `df_bins` keep whatever the ERB mask left there.
pub fn apply_deep_filter(
    out: &mut Spectrum,
    history: &[Spectrum],
    coefficients: &[f32],
    df_bins: usize,
    df_order: usize,
) {
    let bins = df_bins.min(out.len());
    // Taps are indexed from the newest frame backwards, so that a short history
    // drops the oldest taps rather than shifting all of them.
    let available = history.len().min(df_order);

    for bin in 0..bins {
        let mut accumulated = Complex32::default();
        for age in 0..available {
            let tap = df_order - 1 - age;
            let frame = &history[history.len() - 1 - age];
            let base = (bin * df_order + tap) * 2;
            let coefficient = Complex32::new(coefficients[base], coefficients[base + 1]);
            accumulated += coefficient * frame.bin(bin);
        }
        out.set_bin(bin, accumulated);
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_band_gains, apply_deep_filter};
    use noican_core::{Complex32, Spectrum};

    fn constant(bins: usize, value: Complex32) -> Spectrum {
        let mut spectrum = Spectrum::zeroed(bins);
        for index in 0..bins {
            spectrum.set_bin(index, value);
        }
        spectrum
    }

    #[test]
    fn band_gains_reach_every_bin_of_their_band() {
        let mut spectrum = constant(6, Complex32::new(1.0, 0.0));
        apply_band_gains(&mut spectrum, &[0.5, 2.0], &[2, 4]);
        for index in 0..2 {
            assert!((spectrum.bin(index).re - 0.5).abs() < 1e-6, "bin {index}");
        }
        for index in 2..6 {
            assert!((spectrum.bin(index).re - 2.0).abs() < 1e-6, "bin {index}");
        }
    }

    /// Band widths come from the ERB layout and always tile the spectrum, but a
    /// mismatched pair must not write out of bounds.
    #[test]
    fn band_gains_stop_at_the_end_of_the_spectrum() {
        let mut spectrum = constant(3, Complex32::new(1.0, 0.0));
        apply_band_gains(&mut spectrum, &[2.0, 3.0], &[2, 8]);
        assert!((spectrum.bin(2).re - 3.0).abs() < 1e-6);
    }

    /// Tap 0 pairs with the oldest frame. Reversing that is a 17 dB mistake, so
    /// the test uses a filter that only picks one frame and checks which.
    #[test]
    fn tap_zero_selects_the_oldest_frame() {
        let order = 3;
        let history = vec![
            constant(2, Complex32::new(1.0, 0.0)),
            constant(2, Complex32::new(2.0, 0.0)),
            constant(2, Complex32::new(4.0, 0.0)),
        ];
        // Bin 0: only tap 0 is non-zero. Bin 1: only the last tap.
        let coefficients = vec![
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, // bin 0
            0.0, 0.0, 0.0, 0.0, 1.0, 0.0, // bin 1
        ];
        let mut out = Spectrum::zeroed(2);
        apply_deep_filter(&mut out, &history, &coefficients, 2, order);
        assert!(
            (out.bin(0).re - 1.0).abs() < 1e-6,
            "tap 0 is not the oldest"
        );
        assert!(
            (out.bin(1).re - 4.0).abs() < 1e-6,
            "the last tap is not the newest"
        );
    }

    /// Complex is the trailing axis, so `[re, im]` pairs per tap. Getting this
    /// wrong scrambles the phase and costs 6 dB.
    #[test]
    fn coefficients_are_complex_on_the_trailing_axis() {
        let history = vec![constant(1, Complex32::new(0.0, 1.0))];
        // A single tap of (0, 1): multiplying i by i gives -1.
        let coefficients = vec![0.0, 1.0];
        let mut out = Spectrum::zeroed(1);
        apply_deep_filter(&mut out, &history, &coefficients, 1, 1);
        assert!((out.bin(0).re + 1.0).abs() < 1e-6, "got {:?}", out.bin(0));
        assert!(out.bin(0).im.abs() < 1e-6);
    }

    #[test]
    fn only_the_low_bins_are_replaced() {
        let history = vec![constant(4, Complex32::new(1.0, 0.0))];
        let mut out = constant(4, Complex32::new(9.0, 0.0));
        // A zero filter, so any bin it touches becomes zero.
        apply_deep_filter(&mut out, &history, &[0.0, 0.0, 0.0, 0.0], 2, 1);
        assert!(out.bin(0).re.abs() < 1e-9);
        assert!(out.bin(1).re.abs() < 1e-9);
        assert!(
            (out.bin(2).re - 9.0).abs() < 1e-6,
            "a high bin was replaced"
        );
        assert!((out.bin(3).re - 9.0).abs() < 1e-6);
    }

    /// The history can be longer than the filter, in which case the filter uses
    /// its most recent `df_order` frames.
    #[test]
    fn a_longer_history_uses_its_newest_frames() {
        let history = vec![
            constant(1, Complex32::new(1.0, 0.0)),
            constant(1, Complex32::new(2.0, 0.0)),
            constant(1, Complex32::new(4.0, 0.0)),
        ];
        let mut out = Spectrum::zeroed(1);
        // Two taps, both unity: 2 + 4, not 1 + 2.
        apply_deep_filter(&mut out, &history, &[1.0, 0.0, 1.0, 0.0], 1, 2);
        assert!((out.bin(0).re - 6.0).abs() < 1e-6, "got {:?}", out.bin(0));
    }
}
