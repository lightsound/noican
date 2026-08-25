//! Log-mel filterbank features for the speaker-embedding model.
//!
//! Framing follows Kaldi's conventions, which is what speaker-embedding exports
//! are almost always trained against: 25 ms frames every 10 ms, no padding at
//! the edges, DC removed and pre-emphasis applied per frame, a Povey window, and
//! a power spectrum folded into 80 triangular mel bands.
//!
//! # One deviation, and why
//!
//! Kaldi takes the *natural* logarithm of each band. This export wants
//! decibels. That is not a guess — the two differ only by a factor of 4.34, and
//! the graph normalises its own input mean, so a scale factor is the kind of
//! thing that should not matter and does:
//!
//! | Band scaling | Same-speaker cosine | Different-speaker cosine | Margin |
//! |---|---|---|---|
//! | Natural log | 0.824 | 0.527 | +0.055 |
//! | **Decibels** | **0.590** | **0.018** | **+0.256** |
//!
//! The mean similarity is lower with decibels and the *separation* is far
//! better, which is the only property a gate cares about.

use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Frame length, in samples at 16 kHz — Kaldi's 25 ms default.
pub const FRAME_LENGTH: usize = 400;

/// Frame shift, in samples at 16 kHz — Kaldi's 10 ms default.
pub const FRAME_SHIFT: usize = 160;

/// Number of mel bands the model consumes.
pub const MEL_BANDS: usize = 80;

/// Sample rate the model and these constants assume.
pub const SAMPLE_RATE: u32 = 16_000;

/// Kaldi's default pre-emphasis coefficient.
const PRE_EMPHASIS: f32 = 0.97;

/// Floor applied before taking the logarithm of a band energy.
const ENERGY_FLOOR: f32 = 1e-10;

/// Converts hertz to the HTK mel scale.
fn hertz_to_mel(hertz: f32) -> f32 {
    2595.0 * (1.0 + hertz / 700.0).log10()
}

/// Converts an HTK mel value back to hertz.
fn mel_to_hertz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

/// Triangular mel filterbank, `MEL_BANDS` rows of `FRAME_LENGTH / 2 + 1`.
fn mel_filterbank() -> Vec<Vec<f32>> {
    let bins = FRAME_LENGTH / 2 + 1;
    #[expect(
        clippy::cast_precision_loss,
        reason = "audio sample rates are exact in f32"
    )]
    let nyquist = SAMPLE_RATE as f32 / 2.0;

    let low = hertz_to_mel(0.0);
    let high = hertz_to_mel(nyquist);
    #[expect(
        clippy::cast_precision_loss,
        reason = "band counts are small integers, exact in f32"
    )]
    let step = (high - low) / (MEL_BANDS + 1) as f32;
    let edges: Vec<f32> = (0..MEL_BANDS + 2)
        .map(|index| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "index is bounded by the band count"
            )]
            let position = index as f32;
            mel_to_hertz(position.mul_add(step, low))
        })
        .collect();

    #[expect(
        clippy::cast_precision_loss,
        reason = "bin counts are small integers, exact in f32"
    )]
    let bin_width = nyquist / (bins - 1) as f32;
    (0..MEL_BANDS)
        .map(|band| {
            let (low, centre, high) = (edges[band], edges[band + 1], edges[band + 2]);
            (0..bins)
                .map(|bin| {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "bin index is bounded by the transform size"
                    )]
                    let frequency = bin as f32 * bin_width;
                    let rising = (frequency - low) / (centre - low).max(f32::EPSILON);
                    let falling = (high - frequency) / (high - centre).max(f32::EPSILON);
                    rising.min(falling).clamp(0.0, 1.0)
                })
                .collect()
        })
        .collect()
}

/// Povey window: a Hann window raised to the 0.85 power.
fn povey_window() -> Vec<f32> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame lengths are small integers, exact in f32"
    )]
    let span = (FRAME_LENGTH - 1) as f32;
    (0..FRAME_LENGTH)
        .map(|index| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "index is bounded by the frame length"
            )]
            let position = index as f32;
            let hann = 0.5f32.mul_add(-(std::f32::consts::TAU * position / span).cos(), 0.5);
            hann.powf(0.85)
        })
        .collect()
}

/// Computes log-mel features for whole buffers of 16 kHz audio.
pub struct LogMelFbank {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    filterbank: Vec<Vec<f32>>,
    frame: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex32>,
    scratch: Vec<realfft::num_complex::Complex32>,
}

impl std::fmt::Debug for LogMelFbank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogMelFbank")
            .field("frame_length", &FRAME_LENGTH)
            .field("bands", &MEL_BANDS)
            .finish_non_exhaustive()
    }
}

impl Default for LogMelFbank {
    fn default() -> Self {
        Self::new()
    }
}

impl LogMelFbank {
    /// Builds the feature extractor.
    #[must_use]
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME_LENGTH);
        let spectrum = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self {
            fft,
            window: povey_window(),
            filterbank: mel_filterbank(),
            frame: vec![0.0; FRAME_LENGTH],
            spectrum,
            scratch,
        }
    }

    /// Number of frames `samples` will produce.
    ///
    /// Zero when the buffer is shorter than one frame; Kaldi's `snip_edges`
    /// behaviour, which drops the partial frame rather than padding it.
    #[must_use]
    pub const fn frame_count(samples: usize) -> usize {
        if samples < FRAME_LENGTH {
            0
        } else {
            (samples - FRAME_LENGTH) / FRAME_SHIFT + 1
        }
    }

    /// Computes features for `samples`, appending `MEL_BANDS` values per frame.
    ///
    /// Returns the number of frames written. `out` is cleared first.
    pub fn compute(&mut self, samples: &[f32], out: &mut Vec<f32>) -> usize {
        out.clear();
        let frames = Self::frame_count(samples.len());
        out.reserve(frames * MEL_BANDS);

        for index in 0..frames {
            let start = index * FRAME_SHIFT;
            self.frame
                .copy_from_slice(&samples[start..start + FRAME_LENGTH]);

            #[expect(
                clippy::cast_precision_loss,
                reason = "frame lengths are small integers, exact in f32"
            )]
            let length = FRAME_LENGTH as f32;
            let mean = self.frame.iter().sum::<f32>() / length;
            for value in &mut self.frame {
                *value -= mean;
            }

            // Pre-emphasis, in place and backwards so each sample still sees its
            // unmodified predecessor.
            for position in (1..FRAME_LENGTH).rev() {
                self.frame[position] =
                    PRE_EMPHASIS.mul_add(-self.frame[position - 1], self.frame[position]);
            }
            self.frame[0] *= 1.0 - PRE_EMPHASIS;

            for (value, weight) in self.frame.iter_mut().zip(&self.window) {
                *value *= weight;
            }

            // `process_with_scratch` overwrites its input, which is why `frame`
            // is rebuilt from `samples` every iteration rather than reused.
            let _ = self.fft.process_with_scratch(
                &mut self.frame,
                &mut self.spectrum,
                &mut self.scratch,
            );

            for band in &self.filterbank {
                let mut energy = 0.0f32;
                for (weight, bin) in band.iter().zip(&self.spectrum) {
                    if *weight > 0.0 {
                        energy += weight * bin.norm_sqr();
                    }
                }
                out.push(10.0 * energy.max(ENERGY_FLOOR).log10());
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FRAME_LENGTH, FRAME_SHIFT, LogMelFbank, MEL_BANDS, hertz_to_mel, mel_filterbank,
        mel_to_hertz, povey_window,
    };

    #[test]
    fn the_mel_scale_round_trips() {
        for hertz in [0.0, 100.0, 1_000.0, 4_000.0, 8_000.0] {
            let round_tripped = mel_to_hertz(hertz_to_mel(hertz));
            assert!(
                (round_tripped - hertz).abs() < 0.1,
                "{hertz} Hz came back as {round_tripped}"
            );
        }
    }

    #[test]
    fn bands_are_ordered_and_widen_with_frequency() {
        let bank = mel_filterbank();
        assert_eq!(bank.len(), MEL_BANDS);

        let width = |band: &Vec<f32>| band.iter().filter(|weight| **weight > 0.0).count();
        let low = width(&bank[0]);
        let high = width(&bank[MEL_BANDS - 1]);
        assert!(low >= 1, "the lowest band is empty");
        assert!(
            high > low,
            "bands do not widen: {low} bins at the bottom, {high} at the top"
        );

        let peak = |band: &Vec<f32>| {
            band.iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(index, _)| index)
                .unwrap()
        };
        let mut previous = 0;
        for (index, band) in bank.iter().enumerate() {
            let centre = peak(band);
            assert!(
                centre >= previous,
                "band {index} peaks below its predecessor"
            );
            previous = centre;
        }
    }

    #[test]
    fn the_window_is_symmetric_and_bounded() {
        let window = povey_window();
        assert_eq!(window.len(), FRAME_LENGTH);
        for (front, back) in window.iter().zip(window.iter().rev()) {
            assert!((front - back).abs() < 1e-6, "the window is asymmetric");
        }
        assert!(window.iter().all(|value| (0.0..=1.0).contains(value)));
        // Hann^0.85 peaks at 1.0 in the middle.
        assert!((window[FRAME_LENGTH / 2] - 1.0).abs() < 1e-3);
    }

    /// Kaldi's `snip_edges`: the partial frame at the end is dropped, not padded.
    #[test]
    fn frame_counts_follow_kaldi_snip_edges() {
        assert_eq!(LogMelFbank::frame_count(0), 0);
        assert_eq!(LogMelFbank::frame_count(FRAME_LENGTH - 1), 0);
        assert_eq!(LogMelFbank::frame_count(FRAME_LENGTH), 1);
        assert_eq!(LogMelFbank::frame_count(FRAME_LENGTH + FRAME_SHIFT - 1), 1);
        assert_eq!(LogMelFbank::frame_count(FRAME_LENGTH + FRAME_SHIFT), 2);
        // One second at 16 kHz.
        assert_eq!(LogMelFbank::frame_count(16_000), 98);
    }

    #[test]
    fn silence_sits_at_the_energy_floor() {
        let mut fbank = LogMelFbank::new();
        let mut features = Vec::new();
        let frames = fbank.compute(&vec![0.0; 16_000], &mut features);
        assert_eq!(frames, 98);
        assert_eq!(features.len(), frames * MEL_BANDS);
        // 10 * log10(1e-10) is -100 dB.
        for value in &features {
            assert!((value + 100.0).abs() < 1e-3, "silence gave {value} dB");
        }
    }

    #[test]
    fn a_tone_lands_in_the_band_that_contains_it() {
        let mut fbank = LogMelFbank::new();
        let mut features = Vec::new();
        let tone: Vec<f32> = (0..16_000)
            .map(|index| {
                #[expect(clippy::cast_precision_loss, reason = "test fixture")]
                let time = index as f32 / 16_000.0;
                (std::f32::consts::TAU * 1_000.0 * time).sin() * 0.5
            })
            .collect();
        let frames = fbank.compute(&tone, &mut features);

        // Take a frame from the middle, well past the window's edge effects.
        let frame = &features[(frames / 2) * MEL_BANDS..(frames / 2 + 1) * MEL_BANDS];
        let loudest = frame
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(index, _)| index)
            .unwrap();

        let bank = mel_filterbank();
        // Bin 25 of a 400-point transform at 16 kHz is 1 kHz.
        assert!(
            bank[loudest][25] > 0.0,
            "a 1 kHz tone peaked in band {loudest}, which does not cover 1 kHz"
        );
    }

    #[test]
    fn features_scale_with_level_but_bands_keep_their_shape() {
        let mut fbank = LogMelFbank::new();
        let noise: Vec<f32> = (0..8_000i32)
            .map(|index: i32| {
                let x = (f64::from(index) * 12.9898).sin() * 43_758.545_3;
                #[expect(clippy::cast_possible_truncation, reason = "test fixture")]
                let fraction = (x - x.floor()) as f32;
                fraction.mul_add(2.0, -1.0) * 0.25
            })
            .collect();

        let mut quiet = Vec::new();
        fbank.compute(&noise, &mut quiet);
        let louder: Vec<f32> = noise.iter().map(|value| value * 10.0).collect();
        let mut loud = Vec::new();
        fbank.compute(&louder, &mut loud);

        // Ten times the amplitude is a hundred times the power, so every band
        // should rise by 20 dB and the shape across bands should be unchanged.
        for (quiet, loud) in quiet.iter().zip(&loud) {
            assert!(
                (loud - quiet - 20.0).abs() < 1e-2,
                "expected a uniform 20 dB rise, got {}",
                loud - quiet
            );
        }
    }
}
