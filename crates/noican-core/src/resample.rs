//! Polyphase rational resampling.
//!
//! Models run at 16 kHz or 48 kHz while the host path is fixed at 48 kHz, so
//! every stage whose native rate differs needs conversion on both sides. The
//! ratio is always rational (and usually a small integer), which lets us use a
//! fixed polyphase FIR: no interpolation of filter coefficients at run time, no
//! allocation, and a group delay that is known exactly.

use crate::error::{Error, Result};

/// Half-length of each polyphase branch, in input-rate samples.
///
/// 128 gives a ~800 Hz transition band at 48 kHz with the Kaiser window below,
/// which places the stop band just above the 8 kHz Nyquist of the 16 kHz
/// models. Cost is 257 multiply-adds per output sample — negligible next to the
/// neural network it feeds.
const HALF_LENGTH: usize = 128;

/// Taps per polyphase branch.
const TAPS_PER_PHASE: usize = 2 * HALF_LENGTH + 1;

/// Fraction of the lower Nyquist frequency the pass band is allowed to reach.
const CUTOFF_FRACTION: f32 = 0.90;

/// Kaiser `beta` for roughly 70 dB of stop-band attenuation.
const KAISER_BETA: f32 = 6.7554;

/// Resamples a mono stream between two rates whose ratio is rational.
///
/// The converter is streaming: feed it any number of input samples and it
/// produces however many output samples the ratio allows, carrying the
/// fractional phase across calls.
#[derive(Debug)]
pub struct RationalResampler {
    input_rate: u32,
    output_rate: u32,
    /// Interpolation factor, `output_rate / gcd`.
    up: usize,
    /// Decimation factor, `input_rate / gcd`.
    down: usize,
    /// `up` branches of `TAPS_PER_PHASE` taps each, laid out branch-major.
    taps: Box<[f32]>,
    /// Delay line holding the most recent `TAPS_PER_PHASE` input samples.
    history: Box<[f32]>,
    /// Index in `history` at which the next input sample will be written; also
    /// one past the newest sample.
    write: usize,
    /// Polyphase counter: which branch the next output uses, stepped by `down`
    /// and folded back by `up` once per input sample.
    counter: usize,
}

impl RationalResampler {
    /// Builds a resampler from `input_rate` to `output_rate`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfiguration`] if either rate is zero.
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self> {
        if input_rate == 0 || output_rate == 0 {
            return Err(Error::InvalidConfiguration(format!(
                "sample rates must be non-zero (got {input_rate} -> {output_rate})"
            )));
        }

        let divisor = gcd(input_rate, output_rate);
        let up = (output_rate / divisor) as usize;
        let down = (input_rate / divisor) as usize;
        let identity = input_rate == output_rate;

        Ok(Self {
            input_rate,
            output_rate,
            up,
            down,
            // An identity conversion never convolves, so skip the design work.
            taps: if identity {
                Vec::new().into_boxed_slice()
            } else {
                design_polyphase(up, input_rate, output_rate)
            },
            history: vec![0.0; TAPS_PER_PHASE].into_boxed_slice(),
            write: 0,
            counter: 0,
        })
    }

    /// Input sample rate in hertz.
    #[must_use]
    pub const fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// Output sample rate in hertz.
    #[must_use]
    pub const fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// Whether this converter passes samples through untouched.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.input_rate == self.output_rate
    }

    /// Index of the prototype filter's centre tap.
    ///
    /// Output sample `m` reproduces input time `(m * down - centre) / up`, so
    /// the centre index is all that is needed to express the group delay in
    /// either rate.
    const fn centre_index(&self) -> usize {
        (TAPS_PER_PHASE * self.up - 1) / 2
    }

    /// Group delay of the anti-aliasing filter, expressed in input-rate
    /// samples.
    #[must_use]
    pub const fn group_delay_input_samples(&self) -> usize {
        if self.is_identity() {
            0
        } else {
            self.centre_index() / self.up
        }
    }

    /// Group delay of the anti-aliasing filter, expressed in output-rate
    /// samples.
    ///
    /// This is not simply [`Self::group_delay_input_samples`] scaled by the
    /// rate ratio: interpolating by `up` places the centre tap between input
    /// samples, and the two expressions round differently.
    #[must_use]
    pub const fn group_delay_output_samples(&self) -> usize {
        if self.is_identity() {
            0
        } else {
            self.centre_index() / self.down
        }
    }

    /// Upper bound on the number of output samples that `input_len` input
    /// samples can produce, for sizing destination buffers.
    #[must_use]
    pub const fn max_output_len(&self, input_len: usize) -> usize {
        if self.is_identity() {
            return input_len;
        }
        (input_len * self.up).div_ceil(self.down) + 1
    }

    /// Clears the delay line and resets the phase.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.write = 0;
        self.counter = 0;
    }

    /// Converts `input` into `output`, returning how many samples were written.
    ///
    /// Real-time safe: no allocation, no locks, no I/O.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BufferLength`] if `output` is smaller than
    /// [`Self::max_output_len`].
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<usize> {
        let required = self.max_output_len(input.len());
        if output.len() < required {
            return Err(Error::BufferLength {
                expected: required,
                actual: output.len(),
            });
        }

        if self.is_identity() {
            output[..input.len()].copy_from_slice(input);
            return Ok(input.len());
        }

        let mut written = 0;
        for &sample in input {
            self.history[self.write] = sample;
            self.write = (self.write + 1) % TAPS_PER_PHASE;

            while self.counter < self.up {
                output[written] = self.convolve(self.counter);
                written += 1;
                self.counter += self.down;
            }
            self.counter -= self.up;
        }

        Ok(written)
    }

    /// Applies polyphase branch `phase` to the delay line, newest sample first.
    fn convolve(&self, phase: usize) -> f32 {
        let branch = &self.taps[phase * TAPS_PER_PHASE..(phase + 1) * TAPS_PER_PHASE];
        let mut acc = 0.0;
        let mut index = self.write;
        for &tap in branch {
            index = if index == 0 {
                TAPS_PER_PHASE - 1
            } else {
                index - 1
            };
            acc = tap.mul_add(self.history[index], acc);
        }
        acc
    }
}

/// Greatest common divisor, used to reduce the rate ratio.
const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Designs the `up` polyphase branches of a Kaiser-windowed sinc low-pass.
///
/// The prototype conceptually runs at `input_rate * up`, the rate after zero
/// insertion, and cuts off below the lower of the two Nyquist frequencies.
fn design_polyphase(up: usize, input_rate: u32, output_rate: u32) -> Box<[f32]> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "audio sample rates are far below f32's exact-integer limit"
    )]
    let (input_rate, output_rate) = (input_rate as f32, output_rate as f32);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the interpolation factor of an audio rate ratio is a small integer"
    )]
    let up_f = up as f32;

    let interpolated_rate = input_rate * up_f;
    let cutoff = 0.5 * input_rate.min(output_rate) * CUTOFF_FRACTION;
    let normalised_cutoff = cutoff / interpolated_rate;

    let total_taps = TAPS_PER_PHASE * up;
    #[expect(
        clippy::cast_precision_loss,
        reason = "filter lengths are small integers, exact in f32"
    )]
    let last_index = (total_taps - 1) as f32;
    let center = last_index / 2.0;

    let mut prototype = vec![0.0f32; total_taps];
    for (index, tap) in prototype.iter_mut().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "filter tap index is bounded by the filter length"
        )]
        let index_f = index as f32;
        let position = index_f - center;
        let sinc = 2.0 * normalised_cutoff * sinc(2.0 * normalised_cutoff * position);
        *tap = sinc * kaiser(2.0 * index_f / last_index - 1.0) * up_f;
    }

    // Deinterleave into branch-major order so `convolve` reads contiguously.
    let mut branches = vec![0.0f32; total_taps];
    for phase in 0..up {
        for k in 0..TAPS_PER_PHASE {
            branches[phase * TAPS_PER_PHASE + k] = prototype[phase + k * up];
        }
    }
    branches.into_boxed_slice()
}

/// Normalised sinc, `sin(pi x) / (pi x)`.
fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 {
        1.0
    } else {
        let pi_x = core::f32::consts::PI * x;
        pi_x.sin() / pi_x
    }
}

/// Kaiser window evaluated at `ratio` in `[-1, 1]`.
fn kaiser(ratio: f32) -> f32 {
    let arg = ratio.mul_add(-ratio, 1.0);
    if arg <= 0.0 {
        return 0.0;
    }
    bessel_i0(KAISER_BETA * arg.sqrt()) / bessel_i0(KAISER_BETA)
}

/// Modified Bessel function of the first kind, order zero, via its power series.
///
/// The series converges quickly for the small arguments a Kaiser window needs.
fn bessel_i0(x: f32) -> f32 {
    let mut sum = 1.0f32;
    let mut term = 1.0f32;
    let half_x_squared = (x / 2.0) * (x / 2.0);
    for k in 1..=24u32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "the loop bound is 24, exact in f32"
        )]
        let k_f = k as f32;
        term *= half_x_squared / (k_f * k_f);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::{HALF_LENGTH, RationalResampler};

    fn sine(rate: u32, freq: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "test fixture")]
                let t = i as f32 / rate as f32;
                (2.0 * core::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    fn convert(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
        let mut resampler = RationalResampler::new(from, to).unwrap();
        let mut output = vec![0.0; resampler.max_output_len(input.len())];
        let written = resampler.process(input, &mut output).unwrap();
        output.truncate(written);
        output
    }

    #[test]
    fn identity_ratio_copies_input() {
        let resampler = RationalResampler::new(48_000, 48_000).unwrap();
        assert!(resampler.is_identity());
        assert_eq!(resampler.group_delay_input_samples(), 0);
        assert_eq!(
            convert(48_000, 48_000, &[0.1, 0.2, 0.3]),
            vec![0.1, 0.2, 0.3]
        );
    }

    #[test]
    fn downsampling_produces_one_third_of_the_samples() {
        assert_eq!(convert(48_000, 16_000, &vec![0.0; 4_800]).len(), 1_600);
    }

    #[test]
    fn upsampling_produces_three_times_the_samples() {
        assert_eq!(convert(16_000, 48_000, &vec![0.0; 1_600]).len(), 4_800);
    }

    #[test]
    fn non_integer_ratio_tracks_the_rate() {
        let written = convert(44_100, 48_000, &vec![0.0; 44_100]).len();
        assert!(written.abs_diff(48_000) <= 2, "written = {written}");
    }

    #[test]
    fn round_trip_preserves_an_in_band_tone() {
        let input = sine(48_000, 440.0, 24_000);
        let down = RationalResampler::new(48_000, 16_000).unwrap();
        let up = RationalResampler::new(16_000, 48_000).unwrap();

        let mid = convert(48_000, 16_000, &input);
        let output = convert(16_000, 48_000, &mid);

        // Both halves of the delay are expressed at 48 kHz: the decimator's in
        // its input rate, the interpolator's in its output rate.
        let delay = down.group_delay_input_samples() + up.group_delay_output_samples();
        assert_eq!(delay, 513);

        let compare_len = 12_000;
        assert!(output.len() >= delay + compare_len);

        #[expect(clippy::cast_precision_loss, reason = "test fixture")]
        let error = (0..compare_len)
            .map(|i| (output[i + delay] - input[i]).powi(2))
            .sum::<f32>()
            / compare_len as f32;
        assert!(error < 1e-5, "round-trip mean squared error = {error}");
    }

    #[test]
    fn group_delay_is_reported_in_both_rates() {
        let down = RationalResampler::new(48_000, 16_000).unwrap();
        assert_eq!(down.group_delay_input_samples(), HALF_LENGTH);
        assert_eq!(down.group_delay_output_samples(), HALF_LENGTH / 3);

        let up = RationalResampler::new(16_000, 48_000).unwrap();
        assert_eq!(up.group_delay_input_samples(), HALF_LENGTH);
        assert_eq!(up.group_delay_output_samples(), 385);

        let identity = RationalResampler::new(48_000, 48_000).unwrap();
        assert_eq!(identity.group_delay_input_samples(), 0);
        assert_eq!(identity.group_delay_output_samples(), 0);
    }

    #[test]
    fn out_of_band_tone_is_attenuated() {
        // 12 kHz cannot survive a trip through 16 kHz.
        let mid = convert(48_000, 16_000, &sine(48_000, 12_000.0, 24_000));
        let tail = &mid[mid.len() / 2..];
        let peak = tail.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(peak < 0.01, "12 kHz leaked through at {peak}");
    }

    #[test]
    fn rejects_zero_rates() {
        assert!(RationalResampler::new(0, 48_000).is_err());
        assert!(RationalResampler::new(48_000, 0).is_err());
    }

    #[test]
    fn rejects_undersized_output_buffer() {
        let mut resampler = RationalResampler::new(48_000, 16_000).unwrap();
        let mut output = [0.0; 1];
        assert!(resampler.process(&[0.0; 480], &mut output).is_err());
    }

    #[test]
    fn streaming_matches_single_shot() {
        let input = sine(48_000, 1_000.0, 9_600);
        let expected = convert(48_000, 16_000, &input);

        let mut chunked = RationalResampler::new(48_000, 16_000).unwrap();
        let mut actual = Vec::new();
        let mut scratch = vec![0.0; chunked.max_output_len(128)];
        for chunk in input.chunks(128) {
            let written = chunked.process(chunk, &mut scratch).unwrap();
            actual.extend_from_slice(&scratch[..written]);
        }

        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(&expected) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn reset_clears_the_delay_line() {
        let mut resampler = RationalResampler::new(48_000, 16_000).unwrap();
        let mut output = vec![0.0; resampler.max_output_len(480)];
        resampler.process(&[1.0; 480], &mut output).unwrap();
        resampler.reset();

        let mut after = vec![0.0; resampler.max_output_len(480)];
        let written = resampler.process(&[0.0; 480], &mut after).unwrap();
        assert!(after[..written].iter().all(|s| s.abs() < 1e-9));
    }
}
