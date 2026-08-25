//! Analysis windows used by the STFT front-ends.
//!
//! Each published model was trained with one specific window, so the window is
//! part of the model contract rather than a tunable: `DeepFilterNet`-family
//! models (including `DPDFNet`) use the Vorbis window, GTCRN uses a square-root
//! Hann, and UL-UNAS uses a plain Hann.

use core::f32::consts::PI;

/// The analysis windows noican knows how to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Vorbis power-complementary window, `sin(pi/2 * sin^2(pi (n + 0.5) / N))`.
    ///
    /// Used by `DeepFilterNet` and `DPDFNet`. Satisfies the Princen-Bradley
    /// condition, so analysis and synthesis can share it.
    Vorbis,

    /// Periodic Hann window.
    Hann,

    /// Element-wise square root of the periodic Hann window.
    ///
    /// The usual weighted-overlap-add pair at 50 % overlap: analysis and
    /// synthesis both use it, and their product is a Hann window.
    HannSqrt,
}

impl WindowKind {
    /// Builds a window of `length` samples.
    #[must_use]
    pub fn build(self, length: usize) -> Vec<f32> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "window lengths are small powers-of-two-ish integers, exact in f32"
        )]
        let n = length as f32;
        (0..length)
            .map(|i| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "window index is bounded by the window length"
                )]
                let idx = i as f32;
                match self {
                    Self::Vorbis => {
                        let s = (0.5 * PI * (idx + 0.5) / (n / 2.0)).sin();
                        (0.5 * PI * s * s).sin()
                    }
                    Self::Hann => hann(idx, n),
                    Self::HannSqrt => hann(idx, n).sqrt(),
                }
            })
            .collect()
    }

    /// The name used for this window in ONNX model metadata.
    #[must_use]
    pub const fn onnx_metadata_name(self) -> &'static str {
        match self {
            Self::Vorbis => "vorbis",
            Self::Hann => "hann",
            Self::HannSqrt => "hann_sqrt",
        }
    }

    /// Parses an ONNX `window_type` metadata value.
    #[must_use]
    pub fn from_onnx_metadata_name(name: &str) -> Option<Self> {
        match name {
            "vorbis" => Some(Self::Vorbis),
            "hann" | "hanning" => Some(Self::Hann),
            "hann_sqrt" | "sqrt_hann" => Some(Self::HannSqrt),
            _ => None,
        }
    }
}

/// One sample of a periodic Hann window of length `n` at position `idx`.
fn hann(idx: f32, n: f32) -> f32 {
    0.5f32.mul_add(-(2.0 * PI * idx / n).cos(), 0.5)
}

#[cfg(test)]
mod tests {
    use super::WindowKind;

    /// A window `w` with hop `N/2` reconstructs exactly when
    /// `w[n]^2 + w[n + N/2]^2 == 1`.
    fn assert_power_complementary(window: &[f32]) {
        let half = window.len() / 2;
        for i in 0..half {
            let sum = window[i].mul_add(window[i], window[i + half] * window[i + half]);
            assert!((sum - 1.0).abs() < 1e-5, "bin {i}: {sum}");
        }
    }

    #[test]
    fn vorbis_is_power_complementary() {
        assert_power_complementary(&WindowKind::Vorbis.build(960));
    }

    #[test]
    fn hann_sqrt_is_power_complementary() {
        assert_power_complementary(&WindowKind::HannSqrt.build(512));
    }

    #[test]
    fn hann_starts_at_zero_and_peaks_in_the_middle() {
        let window = WindowKind::Hann.build(512);
        assert!(window[0].abs() < 1e-6);
        assert!((window[256] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn metadata_names_round_trip() {
        for kind in [WindowKind::Vorbis, WindowKind::Hann, WindowKind::HannSqrt] {
            let name = kind.onnx_metadata_name();
            assert_eq!(WindowKind::from_onnx_metadata_name(name), Some(kind));
        }
        assert_eq!(WindowKind::from_onnx_metadata_name("blackman"), None);
    }
}
