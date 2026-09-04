//! Streaming polyphase FIR resampling.
//!
//! Two families share one Kaiser-windowed sinc design:
//!
//! - [`Decimator`] / [`Interpolator`]: integer factors for the model path
//!   (the engine runs at 48 kHz; some models run at 16 kHz, 48000 / 16000
//!   = 3). Fixed, tiny, deterministic.
//! - [`PolyphaseResampler`]: arbitrary ratios for the capture path — the
//!   exact rational `L/M` between a microphone's native rate and the
//!   engine rate (160/147 for 44.1 kHz, 3/1 for 16 kHz, 1/2 for 96 kHz),
//!   plus a fractional phase step that folds in the clock-drift
//!   correction, so one stage does both jobs.
//!
//! Everything is allocation-free after construction (safe for a
//! real-time inference thread).

/// Zeroth-order modified Bessel function of the first kind (series
/// expansion), used by the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0_f64;
    let mut term = 1.0_f64;
    let half_x = x / 2.0;
    for k in 1..64 {
        term *= (half_x / f64::from(k)) * (half_x / f64::from(k));
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}

/// Designs a Kaiser-windowed sinc lowpass FIR.
///
/// `cutoff` is in cycles per sample (0 < cutoff < 0.5). The returned filter
/// has unity DC gain.
fn design_kaiser_lowpass(num_taps: usize, cutoff: f64, beta: f64) -> Vec<f32> {
    assert!(num_taps >= 3 && cutoff > 0.0 && cutoff < 0.5);
    #[expect(
        clippy::cast_precision_loss,
        reason = "tap counts are tiny (hundreds); exact f64 representation"
    )]
    let mid = (num_taps - 1) as f64 / 2.0;
    let i0_beta = bessel_i0(beta);
    let mut taps = Vec::with_capacity(num_taps);
    let mut dc_gain = 0.0_f64;
    for i in 0..num_taps {
        #[expect(
            clippy::cast_precision_loss,
            reason = "tap index is tiny; exact f64 representation"
        )]
        let x = i as f64 - mid;
        let sinc = if x == 0.0 {
            2.0 * cutoff
        } else {
            (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
        };
        let frac = x / mid;
        let window = bessel_i0(beta * (1.0 - frac * frac).max(0.0).sqrt()) / i0_beta;
        dc_gain += sinc * window;
        taps.push(sinc * window);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "filter coefficients are within f32 range by construction"
    )]
    taps.iter().map(|t| (t / dc_gain) as f32).collect()
}

/// Number of taps used for a given integer factor. Odd so the group delay is
/// an integer number of samples at the high rate.
const fn taps_for_factor(factor: usize) -> usize {
    40 * factor + 1
}

/// Streaming decimator by an integer factor (input at the high rate, output
/// at the low rate). Inputs must be supplied in multiples of the factor.
#[derive(Debug)]
pub struct Decimator {
    factor: usize,
    taps: Vec<f32>,
    /// Last `taps.len() - 1` input samples from previous calls.
    hist: Vec<f32>,
    work: Vec<f32>,
}

impl Decimator {
    /// Creates a decimator by `factor` (>= 2), pre-allocating for inputs of
    /// up to `max_input_len` samples per call.
    ///
    /// # Panics
    ///
    /// Panics when `factor < 2`.
    #[must_use]
    pub fn new(factor: usize, max_input_len: usize) -> Self {
        assert!(factor >= 2);
        let num_taps = taps_for_factor(factor);
        #[expect(
            clippy::cast_precision_loss,
            reason = "factor is tiny; exact f64 representation"
        )]
        let cutoff = 0.45 / factor as f64;
        let taps = design_kaiser_lowpass(num_taps, cutoff, 9.0);
        Self {
            factor,
            hist: vec![0.0; num_taps - 1],
            work: Vec::with_capacity(num_taps - 1 + max_input_len),
            taps,
        }
    }

    /// Group delay in samples at the (high) input rate.
    #[must_use]
    pub const fn delay_input_samples(&self) -> usize {
        (self.taps.len() - 1) / 2
    }

    /// Consumes `input` (length must be a multiple of the factor) and appends
    /// `input.len() / factor` samples to `output`.
    ///
    /// # Panics
    ///
    /// Panics when `input.len()` is not a multiple of the factor.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        assert_eq!(input.len() % self.factor, 0);
        let hist_len = self.taps.len() - 1;
        self.work.clear();
        self.work.extend_from_slice(&self.hist);
        self.work.extend_from_slice(input);
        for out_idx in 0..input.len() / self.factor {
            // Newest input sample involved in this output sample sits at
            // work[hist_len + out_idx * factor].
            let end = hist_len + out_idx * self.factor;
            let mut acc = 0.0_f32;
            for (k, tap) in self.taps.iter().enumerate() {
                acc = tap.mul_add(self.work[end - k], acc);
            }
            output.push(acc);
        }
        let tail_start = self.work.len() - hist_len;
        self.hist.copy_from_slice(&self.work[tail_start..]);
    }

    /// Clears filter history.
    pub fn reset(&mut self) {
        self.hist.fill(0.0);
    }
}

/// Streaming interpolator by an integer factor (input at the low rate,
/// output at the high rate).
#[derive(Debug)]
pub struct Interpolator {
    /// Polyphase filter bank: `phases[p][m] = taps[m * factor + p] * factor`.
    phases: Vec<Vec<f32>>,
    /// Last `history_len` low-rate input samples.
    hist: Vec<f32>,
    work: Vec<f32>,
    delay_high: usize,
}

impl Interpolator {
    /// Creates an interpolator by `factor` (>= 2), pre-allocating for inputs
    /// of up to `max_input_len` low-rate samples per call.
    ///
    /// # Panics
    ///
    /// Panics when `factor < 2`.
    #[must_use]
    pub fn new(factor: usize, max_input_len: usize) -> Self {
        assert!(factor >= 2);
        let num_taps = taps_for_factor(factor);
        #[expect(
            clippy::cast_precision_loss,
            reason = "factor is tiny; exact f64 representation"
        )]
        let cutoff = 0.45 / factor as f64;
        let taps = design_kaiser_lowpass(num_taps, cutoff, 9.0);
        #[expect(
            clippy::cast_precision_loss,
            reason = "factor is tiny; exact f32 representation"
        )]
        let gain = factor as f32;
        let phase_len = num_taps.div_ceil(factor);
        let mut phases = vec![vec![0.0_f32; phase_len]; factor];
        for (i, tap) in taps.iter().enumerate() {
            phases[i % factor][i / factor] = tap * gain;
        }
        Self {
            phases,
            hist: vec![0.0; phase_len - 1],
            work: Vec::with_capacity(phase_len - 1 + max_input_len),
            delay_high: (num_taps - 1) / 2,
        }
    }

    /// Group delay in samples at the (high) output rate.
    #[must_use]
    pub const fn delay_output_samples(&self) -> usize {
        self.delay_high
    }

    /// Consumes low-rate `input` and appends `input.len() * factor` high-rate
    /// samples to `output`.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let hist_len = self.hist.len();
        self.work.clear();
        self.work.extend_from_slice(&self.hist);
        self.work.extend_from_slice(input);
        for n in 0..input.len() {
            let newest = hist_len + n;
            for phase in &self.phases {
                let mut acc = 0.0_f32;
                for (m, coef) in phase.iter().enumerate() {
                    acc = coef.mul_add(self.work[newest - m], acc);
                }
                output.push(acc);
            }
        }
        let tail_start = self.work.len() - hist_len;
        self.hist.copy_from_slice(&self.work[tail_start..]);
    }

    /// Clears filter history.
    pub fn reset(&mut self) {
        self.hist.fill(0.0);
    }
}

/// Largest drift correction a [`PolyphaseResampler`] applies, in parts
/// per million.
///
/// ±2000 ppm (0.2%) is an order of magnitude above real crystal
/// mismatches, small enough to be inaudible (≈3.5 cents), and bounds how
/// fast a servo can slew the ratio while recovering a priming offset.
pub const MAX_DRIFT_PPM: f64 = 2000.0;

/// Fewest polyphase branches per input sample a [`PolyphaseResampler`]
/// builds. A small upsampling factor (×2, ×3) is oversampled to at least
/// this many phases so that the linear interpolation between adjacent
/// phase filters — which is what makes the ratio *arbitrary* — happens on
/// a grid of at least 128 × the input rate. At 128 phases the
/// interpolation error is that of a linear interpolator running at
/// ≥ 1 MHz: below 0.001 dB of passband droop and below −90 dB of image
/// leakage for any audio-band tone. Measured (1 s sines, 44.1 → 48 kHz,
/// `polyphase_tracks_a_fixed_drift_correction`): 93.6 dB SNR at 1 kHz
/// and 94–97 dB at 15 kHz for both 0 ppm and −500 ppm — the phase
/// interpolation adds no error visible above the filter's own. The cost
/// of more phases is memory only (~40 taps × phases × 4 bytes).
const MIN_PHASES: usize = 128;

/// Half the filter span in samples at the *lower* of the two rates: the
/// same 40-sample windowed-sinc span the integer-factor designs above
/// use (`taps_for_factor`), which the model path has validated at
/// > 50 dB round-trip SNR.
const HALF_SPAN_LOW_RATE: usize = 20;

const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Streaming arbitrary-ratio polyphase FIR resampler.
///
/// Exact rational conversion between two sample rates, with a fractional
/// phase step that folds a parts-per-million drift correction into the
/// same stage.
///
/// # Design (the capture-path resampler decision)
///
/// Three ways to accept 44.1 kHz-family microphones on the split
/// transport were weighed:
///
/// - **Rational `L/M` stage in front of the existing cubic drift
///   stage.** Minimal change, but two cascaded interpolators, and the
///   Catmull-Rom drift stage — fine for telephony content far below
///   Nyquist — droops ≈ 8 dB at 20 kHz at half-sample phase, which a
///   full-band 44.1 kHz microphone would hear as slow amplitude
///   modulation of its top octave as the drift phase cycles.
/// - **`rubato`** (already a CLI dependency for offline conversion).
///   Its asynchronous sinc resampler is the same algorithm as this type,
///   but it would pull the crate and its FFT stack into the engine
///   dylib, its fixed-chunk API needs a staging layer, and its delay
///   reporting is not integer-exact at the output rate.
/// - **This type**: one windowed-sinc polyphase bank sampled at
///   `num_phases` positions per input sample (the exact `L` of the
///   reduced ratio, oversampled to at least [`MIN_PHASES`]), read at a
///   fractional phase step. At zero drift the step is an integer, so the
///   output is the *exact* rational polyphase result (no interpolation
///   at all: 160/147 for 44.1 kHz, 3/1 for 16 kHz, 1/2 for 96 kHz). At
///   nonzero drift, adjacent phase filters are linearly interpolated,
///   which on a ≥ 128× grid is far more accurate than a cubic at the
///   output rate. Integer factors, the 44.1 kHz family, and downsampling
///   from 88.2/96 kHz are all the same code path; the anti-imaging
///   filter and the drift stage are one filter, so there is nothing to
///   cascade, and the group delay is an integer number of output samples
///   by construction (`num_taps - 1 = 2 · step · delay`).
///
/// The third option supersedes the first two — no extra dependency, one
/// stage instead of two, better drift-stage fidelity — so it is the one
/// implemented.
///
/// # Real-time properties
///
/// Everything is allocated in [`PolyphaseResampler::new`]. `process`
/// performs no allocation while each input chunk stays within
/// `max_input_len` (the caller's output `Vec` must be reserved for
/// [`PolyphaseResampler::max_output_len`] samples), holds no locks, and
/// costs two `phase_len`-tap dot products per output sample (one when
/// the drift correction is zero).
#[derive(Debug)]
pub struct PolyphaseResampler {
    /// Reduced ratio: `up` output samples per `down` input samples.
    up: usize,
    down: usize,
    /// Polyphase bank: `phases[p][m] = taps[m * num_phases + p] * num_phases`.
    phases: Vec<Vec<f32>>,
    /// Phase positions per input sample (`up × oversampling`).
    num_phases: usize,
    /// Nominal advance per output sample in phase units
    /// (`down × oversampling`, an integer).
    nominal_step: f64,
    /// Effective advance per output sample: the nominal step divided by
    /// `1 + ppm · 1e-6`.
    step: f64,
    /// FIR history plus the current chunk. The first `phase_len - 1`
    /// slots seed the look-behind at stream start.
    buf: Vec<f32>,
    /// Position of the next output sample in phase units relative to
    /// `buf[0]`; always ≥ `(phase_len - 1) × num_phases` so every tap
    /// has a sample.
    pos: f64,
    delay_output: usize,
}

impl PolyphaseResampler {
    /// Creates a converter from `input_rate` to `output_rate` Hz,
    /// preallocating for input chunks of up to `max_input_len` samples
    /// per [`PolyphaseResampler::process`] call.
    ///
    /// # Panics
    ///
    /// Panics when either rate is zero.
    #[must_use]
    pub fn new(input_rate: u32, output_rate: u32, max_input_len: usize) -> Self {
        assert!(input_rate > 0 && output_rate > 0);
        let divisor = gcd(input_rate as usize, output_rate as usize);
        let up = output_rate as usize / divisor;
        let down = input_rate as usize / divisor;
        let oversampling = MIN_PHASES.div_ceil(up).max(1);
        let num_phases = up * oversampling;
        let step = down * oversampling;
        // The prototype runs at `num_phases × input_rate`; its passband
        // must stop below the lower of the two Nyquist frequencies:
        // 1 / (2 · num_phases) (input) or 1 / (2 · step) (output).
        let widest = num_phases.max(step);
        // Group delay `(num_taps - 1) / 2 = step × delay` prototype
        // samples is exactly `delay` output samples.
        let delay_output = (HALF_SPAN_LOW_RATE * widest).div_ceil(step);
        let num_taps = 2 * step * delay_output + 1;
        #[expect(
            clippy::cast_precision_loss,
            reason = "phase counts are tiny (hundreds); exact f64 representation"
        )]
        let cutoff = 0.45 / widest as f64;
        let taps = design_kaiser_lowpass(num_taps, cutoff, 9.0);
        #[expect(
            clippy::cast_precision_loss,
            reason = "phase counts are tiny (hundreds); exact f32 representation"
        )]
        let gain = num_phases as f32;
        let phase_len = num_taps.div_ceil(num_phases);
        let mut phases = vec![vec![0.0_f32; phase_len]; num_phases];
        for (i, tap) in taps.iter().enumerate() {
            phases[i % num_phases][i / num_phases] = tap * gain;
        }
        let hist_len = phase_len - 1;
        // Retained between calls: the FIR look-behind plus the one
        // look-ahead sample the phase interpolation may need.
        let mut buf = Vec::with_capacity(hist_len + 1 + max_input_len);
        buf.resize(hist_len, 0.0);
        #[expect(
            clippy::cast_precision_loss,
            reason = "step and history are tiny integers; exact f64 representation"
        )]
        let (nominal_step, pos) = (step as f64, (hist_len * num_phases) as f64);
        Self {
            up,
            down,
            phases,
            num_phases,
            nominal_step,
            step: nominal_step,
            buf,
            pos,
            delay_output,
        }
    }

    /// Reduced conversion ratio as `(up, down)`: the stream produces `up`
    /// output samples per `down` input samples at zero drift correction
    /// (160/147 for 44.1 → 48 kHz, 3/1 for 16 → 48 kHz).
    #[must_use]
    pub const fn ratio(&self) -> (usize, usize) {
        (self.up, self.down)
    }

    /// Group delay in samples at the output rate — an integer by
    /// construction.
    #[must_use]
    pub const fn delay_output_samples(&self) -> usize {
        self.delay_output
    }

    /// Upper bound on the samples one `process` call appends for
    /// `input_len` input samples, at the largest drift correction. Reserve
    /// the output `Vec` to this to keep `process` allocation-free.
    #[must_use]
    pub fn max_output_len(&self, input_len: usize) -> usize {
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "chunk lengths are small positive integers"
        )]
        let nominal = (input_len as f64 * self.up as f64 / self.down as f64
            * MAX_DRIFT_PPM.mul_add(1e-6, 1.0))
        .ceil() as usize;
        // Plus the look-ahead sample released from the previous call and
        // one for the ceiling of the fractional phase.
        nominal + 2
    }

    /// Applies a drift correction in parts per million (clamped to
    /// ±[`MAX_DRIFT_PPM`]; non-finite values read as zero). Positive
    /// values stretch the stream — more output samples per input sample.
    /// At exactly zero the phase step is an integer and the conversion is
    /// the exact rational polyphase result.
    pub fn set_drift_ppm(&mut self, ppm: f64) {
        let ppm = if ppm.is_finite() {
            ppm.clamp(-MAX_DRIFT_PPM, MAX_DRIFT_PPM)
        } else {
            0.0
        };
        self.step = self.nominal_step / ppm.mul_add(1e-6, 1.0);
    }

    /// Consumes `input`, appending every output sample whose filter
    /// support is complete to `output` (one input sample of look-ahead
    /// is held back until the next call; the signal itself is delayed by
    /// exactly [`PolyphaseResampler::delay_output_samples`]).
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        let hist_len = self.phases[0].len() - 1;
        let (mut base, mut phase_pos) = self.locate();
        while base + 1 < self.buf.len() {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "phase_pos is kept in 0..num_phases by locate()"
            )]
            let phase = phase_pos.floor() as usize;
            #[expect(clippy::cast_possible_truncation, reason = "the fraction is in 0..1")]
            let weight = (phase_pos - phase.to_f64()) as f32;
            let first = self.dot(phase, base);
            let value = if weight == 0.0 {
                first
            } else {
                let second = if phase + 1 == self.num_phases {
                    self.dot(0, base + 1)
                } else {
                    self.dot(phase + 1, base)
                };
                (second - first).mul_add(weight, first)
            };
            output.push(value);
            self.pos += self.step;
            (base, phase_pos) = self.locate();
        }
        // Drop the consumed prefix, keeping the FIR look-behind of the
        // next output sample (which may lie past the buffered input when
        // downsampling — clamp to the buffer).
        let keep_from = base.saturating_sub(hist_len).min(self.buf.len());
        self.buf.drain(..keep_from);
        self.pos -= (keep_from * self.num_phases).to_f64();
    }

    /// Clears filter history (the drift correction persists).
    pub fn reset(&mut self) {
        let hist_len = self.phases[0].len() - 1;
        self.buf.clear();
        self.buf.resize(hist_len, 0.0);
        self.pos = (hist_len * self.num_phases).to_f64();
    }

    /// Splits `pos` into the newest input sample the next output uses
    /// (`base`, an index into `buf`) and the fractional phase position in
    /// `0.0..num_phases`. Integer positions (zero drift) split exactly;
    /// the guards absorb a floor landing one off from f64 rounding.
    fn locate(&self) -> (usize, f64) {
        let phases = self.num_phases.to_f64();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "pos is non-negative and bounded by buf.len() × num_phases (small)"
        )]
        let mut base = (self.pos / phases).floor() as usize;
        let mut phase_pos = base.to_f64().mul_add(-phases, self.pos);
        // f64 rounding can land the floor one off; the true position is
        // never more than one phase-count away, so one correction suffices.
        if phase_pos < 0.0 {
            base = base.saturating_sub(1);
            phase_pos += phases;
        } else if phase_pos >= phases {
            base += 1;
            phase_pos -= phases;
        }
        (base, phase_pos.clamp(0.0, phases.next_down()))
    }

    /// Dot product of phase filter `phase` against the input ending at
    /// `buf[base]` (newest sample first).
    fn dot(&self, phase: usize, base: usize) -> f32 {
        let mut acc = 0.0_f32;
        for (m, coef) in self.phases[phase].iter().enumerate() {
            acc = coef.mul_add(self.buf[base - m], acc);
        }
        acc
    }
}

/// Exact `usize → f64` for the small integers (buffer indices, phase
/// counts) this module handles.
trait ToF64 {
    fn to_f64(self) -> f64;
}

impl ToF64 for usize {
    fn to_f64(self) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "callers pass buffer indices and phase counts far below 2^53"
        )]
        let value = self as f64;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Amplitude-0.5 sine, phase accumulated in f64 so long high-frequency
    /// test signals (e.g. 30 kHz at 96 kHz for a second) stay exact.
    fn sine(rate: u32, freq: f32, len: usize) -> Vec<f32> {
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "test signal indices are small; samples are cast to f32 by design"
        )]
        (0..len)
            .map(|n| {
                let phase =
                    2.0 * std::f64::consts::PI * f64::from(freq) * n as f64 / f64::from(rate);
                (phase.sin() * 0.5) as f32
            })
            .collect()
    }

    /// Down-then-up by 3 must reproduce a mid-band sine, delayed by the two
    /// filters' group delays, with high fidelity.
    #[test]
    fn round_trip_by_three_preserves_sine() {
        let factor = 3;
        let input = sine(48_000, 1000.0, 14_400);
        let mut decim = Decimator::new(factor, input.len());
        let mut interp = Interpolator::new(factor, input.len() / factor);
        let mut low = Vec::new();
        decim.process(&input, &mut low);
        assert_eq!(low.len(), input.len() / factor);
        let mut high = Vec::new();
        interp.process(&low, &mut high);
        assert_eq!(high.len(), input.len());

        let delay = decim.delay_input_samples() + interp.delay_output_samples();
        let start = delay + 500;
        let mut err_energy = 0.0_f64;
        let mut sig_energy = 0.0_f64;
        for n in start..input.len() {
            let out = f64::from(high[n]);
            let reference = f64::from(input[n - delay]);
            err_energy = (out - reference).mul_add(out - reference, err_energy);
            sig_energy += reference * reference;
        }
        let snr_db = 10.0 * (sig_energy / err_energy).log10();
        assert!(snr_db > 50.0, "SNR too low: {snr_db} dB");
    }

    /// Streaming in small chunks must produce bit-identical output to one
    /// large call.
    #[test]
    fn chunked_streaming_matches_single_call() {
        let factor = 3;
        let input = sine(48_000, 700.0, 4800);
        let mut one = Vec::new();
        Decimator::new(factor, input.len()).process(&input, &mut one);

        let mut chunked = Vec::new();
        let mut decim = Decimator::new(factor, 480);
        for chunk in input.chunks(480) {
            decim.process(chunk, &mut chunked);
        }
        assert_eq!(one, chunked);
    }

    /// Runs `input` (at `input_rate`) through a fresh polyphase resampler
    /// to `output_rate` in chunks of `chunk`, with a fixed drift
    /// correction.
    fn convert(
        input_rate: u32,
        output_rate: u32,
        input: &[f32],
        chunk: usize,
        ppm: f64,
    ) -> Vec<f32> {
        let mut resampler = PolyphaseResampler::new(input_rate, output_rate, chunk);
        resampler.set_drift_ppm(ppm);
        let mut output = Vec::with_capacity(resampler.max_output_len(input.len()));
        for piece in input.chunks(chunk) {
            resampler.process(piece, &mut output);
        }
        output
    }

    /// SNR (dB) of `output` against a `freq` Hz sine of amplitude 0.5 at
    /// `rate`, delayed by `delay` samples, skipping the first `skip`
    /// samples. Any imaging or aliasing product lands in the error term.
    fn sine_snr_db(output: &[f32], rate: u32, freq: f64, delay: usize, skip: usize) -> f64 {
        let mut err = 0.0_f64;
        let mut sig = 0.0_f64;
        for (n, out) in output.iter().enumerate().skip(skip) {
            #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
            let position = (n - delay) as f64;
            let reference =
                (2.0 * std::f64::consts::PI * freq * position / f64::from(rate)).sin() * 0.5;
            err += (f64::from(*out) - reference).powi(2);
            sig += reference * reference;
        }
        10.0 * (sig / err).log10()
    }

    fn rms(samples: &[f32]) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
        let len = samples.len() as f64;
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / len).sqrt()
    }

    /// The ratio reduces exactly and the integer output delay follows
    /// `num_taps - 1 = 2 · step · delay`.
    #[test]
    fn polyphase_reduces_ratios_and_reports_integer_delays() {
        let cases = [
            (44_100_u32, (160_usize, 147_usize)),
            (22_050, (320, 147)),
            (11_025, (640, 147)),
            (16_000, (3, 1)),
            (24_000, (2, 1)),
            (8_000, (6, 1)),
            (32_000, (3, 2)),
            (48_000, (1, 1)),
            (88_200, (80, 147)),
            (96_000, (1, 2)),
        ];
        for (rate, ratio) in cases {
            let resampler = PolyphaseResampler::new(rate, 48_000, 480);
            assert_eq!(resampler.ratio(), ratio, "{rate} Hz");
            // The integer factors keep the delays of the fixed-factor
            // designs (`taps_for_factor`: 40 × factor + 1 taps).
            if ratio.1 == 1 {
                assert_eq!(
                    resampler.delay_output_samples(),
                    (taps_for_factor(ratio.0) - 1) / 2,
                    "{rate} Hz"
                );
            }
        }
        // 44.1 → 48 kHz: 160 phases, step 147, delay ⌈20·160/147⌉ = 22.
        assert_eq!(
            PolyphaseResampler::new(44_100, 48_000, 480).delay_output_samples(),
            22
        );
    }

    /// Over a long stream the output count is `up / down` × the input
    /// count (one look-ahead sample is held back at the end).
    #[test]
    fn polyphase_output_count_follows_the_ratio() {
        for (rate, chunk) in [
            (44_100_u32, 441_usize),
            (22_050, 480),
            (96_000, 512),
            (16_000, 160),
        ] {
            let input = sine(rate, 440.0, rate as usize * 2);
            let output = convert(rate, 48_000, &input, chunk, 0.0);
            let expected = input.len() * 48_000 / rate as usize;
            assert!(
                output.len().abs_diff(expected) <= 3,
                "{rate} Hz: {} outputs, expected ≈{expected}",
                output.len()
            );
        }
    }

    /// Every capture-path ratio must reproduce a 1 kHz sine at the exact
    /// reported delay with > 50 dB SNR — imaging (upsampling) or aliasing
    /// (downsampling) products would all count as error.
    #[test]
    fn polyphase_preserves_a_sine_at_every_capture_rate() {
        for rate in [
            8_000_u32, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000,
        ] {
            let input = sine(rate, 1000.0, rate as usize);
            let delay = PolyphaseResampler::new(rate, 48_000, 480).delay_output_samples();
            let output = convert(rate, 48_000, &input, 480, 0.0);
            let snr = sine_snr_db(&output, 48_000, 1000.0, delay, 2 * delay + 500);
            assert!(snr > 50.0, "{rate} Hz: SNR too low: {snr} dB");
        }
    }

    /// Downsampling must reject content above the output Nyquist: a
    /// 30 kHz tone at 96 kHz (which would alias to 18 kHz at 48 kHz) has
    /// to come out at least 50 dB down, while a 5 kHz tone passes.
    #[test]
    fn polyphase_downsampling_suppresses_aliasing() {
        let above = sine(96_000, 30_000.0, 96_000);
        let output = convert(96_000, 48_000, &above, 480, 0.0);
        let leak_db = 20.0 * (rms(&output[4800..]) / (0.5 / std::f64::consts::SQRT_2)).log10();
        assert!(leak_db < -50.0, "30 kHz tone leaked at {leak_db} dB");

        let inside = sine(96_000, 5000.0, 96_000);
        let delay = PolyphaseResampler::new(96_000, 48_000, 480).delay_output_samples();
        let output = convert(96_000, 48_000, &inside, 480, 0.0);
        let snr = sine_snr_db(&output, 48_000, 5000.0, delay, 2 * delay + 500);
        assert!(snr > 50.0, "5 kHz tone SNR too low: {snr} dB");
    }

    /// Upsampling 44.1 → 48 kHz keeps the passband flat well into the top
    /// octave: a 15 kHz sine (0.68 × the input Nyquist) comes out above
    /// 60 dB SNR. (Measured on this design: ≈94 dB at 15 kHz, 63 dB at
    /// 17 kHz, then the 40-sample-span transition band, −4 dB at 19 kHz.
    /// The roll-off is the shared `HALF_SPAN_LOW_RATE` design, identical
    /// to the model path's 16 kHz decimator relative to its Nyquist.)
    #[test]
    fn polyphase_upsampling_keeps_the_top_of_the_band() {
        let input = sine(44_100, 15_000.0, 44_100);
        let delay = PolyphaseResampler::new(44_100, 48_000, 480).delay_output_samples();
        let output = convert(44_100, 48_000, &input, 480, 0.0);
        let snr = sine_snr_db(&output, 48_000, 15_000.0, delay, 2 * delay + 500);
        assert!(snr > 60.0, "15 kHz tone SNR too low: {snr} dB");
    }

    /// A fixed drift correction time-warps the output by exactly that
    /// ratio: the count follows `1 + ppm`, and the waveform tracks the
    /// analytically warped sine as accurately as the undrifted
    /// conversion — including at 15 kHz, where a Catmull-Rom drift stage
    /// at the output rate would already droop measurably. (Measured:
    /// 93.6 dB at 1 kHz and 94–97 dB at 15 kHz for both 0 and −500 ppm;
    /// the phase interpolation adds no visible error.)
    #[test]
    fn polyphase_tracks_a_fixed_drift_correction() {
        for freq in [1000.0_f64, 15_000.0] {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "test frequencies are integral"
            )]
            let input = sine(44_100, freq as f32, 44_100 * 2);
            for ppm in [-500.0_f64, 500.0] {
                let output = convert(44_100, 48_000, &input, 480, ppm);
                let stretch = ppm.mul_add(1e-6, 1.0);
                #[expect(
                    clippy::cast_precision_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "test sample counts are small and positive"
                )]
                let expected_len = (input.len() as f64 * 48_000.0 / 44_100.0 * stretch) as usize;
                assert!(
                    output.len().abs_diff(expected_len) <= 3,
                    "{ppm} ppm: {} outputs, expected ≈{expected_len}",
                    output.len()
                );
                let delay = PolyphaseResampler::new(44_100, 48_000, 480).delay_output_samples();
                let mut err = 0.0_f64;
                let mut sig = 0.0_f64;
                for (n, out) in output.iter().enumerate().skip(2 * delay + 500) {
                    // Output sample n sits at input time n / stretch; the
                    // filter delay is fixed in input time.
                    #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
                    let position = n as f64 / stretch - delay as f64;
                    let reference =
                        (2.0 * std::f64::consts::PI * freq * position / 48_000.0).sin() * 0.5;
                    err += (f64::from(*out) - reference).powi(2);
                    sig += reference * reference;
                }
                let snr = 10.0 * (sig / err).log10();
                assert!(snr > 60.0, "{freq} Hz at {ppm} ppm: SNR too low: {snr} dB");
            }
        }
    }

    /// Streaming in small, uneven chunks must produce bit-identical
    /// output to one large call, with and without a drift correction
    /// (the position is rebased exactly at every call).
    #[test]
    fn polyphase_chunked_streaming_matches_single_call_under_drift() {
        let input = sine(44_100, 700.0, 22_050);
        for ppm in [0.0_f64, 137.0] {
            let one = convert(44_100, 48_000, &input, input.len(), ppm);
            let chunked = convert(44_100, 48_000, &input, 97, ppm);
            assert_eq!(one, chunked, "{ppm} ppm");
        }
    }

    /// `max_output_len` bounds the per-call output at the largest
    /// correction, so a caller reserving it never reallocates.
    #[test]
    fn polyphase_max_output_len_bounds_every_call() {
        for rate in [8_000_u32, 44_100, 96_000] {
            let mut resampler = PolyphaseResampler::new(rate, 48_000, 480);
            resampler.set_drift_ppm(MAX_DRIFT_PPM);
            let bound = resampler.max_output_len(480);
            let input = sine(rate, 300.0, 480 * 50);
            let mut output = Vec::new();
            for chunk in input.chunks(480) {
                output.clear();
                resampler.process(chunk, &mut output);
                assert!(
                    output.len() <= bound,
                    "{rate} Hz: {} > {bound}",
                    output.len()
                );
            }
        }
    }

    #[test]
    fn polyphase_reset_restarts_the_stream() {
        let input = sine(44_100, 1000.0, 4410);
        let mut resampler = PolyphaseResampler::new(44_100, 48_000, 4410);
        let mut first = Vec::new();
        resampler.process(&input, &mut first);
        resampler.reset();
        let mut second = Vec::new();
        resampler.process(&input, &mut second);
        assert_eq!(first, second);
    }
}
