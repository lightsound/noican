//! Streaming integer-factor polyphase FIR resampling.
//!
//! The engine runs at 48 kHz; some models run at 16 kHz. Only integer factors
//! are needed (48000 / 16000 = 3), so a fixed windowed-sinc design is used
//! instead of a general-purpose resampler: it is deterministic, tiny, and
//! allocation-free after construction (safe for a real-time inference
//! thread).

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, len: usize) -> Vec<f32> {
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        (0..len)
            .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / rate as f32).sin() * 0.5)
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
}
