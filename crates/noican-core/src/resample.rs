//! See docs/adr/0006-resampling.md#resampling-architecture.

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
        let window = bessel_i0(beta * frac.mul_add(-frac, 1.0).max(0.0).sqrt()) / i0_beta;
        dc_gain = sinc.mul_add(window, dc_gain);
        taps.push(sinc * window);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "filter coefficients are within f32 range by construction"
    )]
    taps.iter().map(|t| (t / dc_gain) as f32).collect()
}

const fn taps_for_factor(factor: usize) -> usize {
    40 * factor + 1
}

/// See docs/adr/0006-resampling.md#resampler-input-contracts.
#[derive(Debug)]
pub struct Decimator {
    factor: usize,
    taps: Vec<f32>,
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
    phases: Vec<Vec<f32>>,
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

/// See docs/adr/0006-resampling.md#drift-correction-design.
pub const MAX_DRIFT_PPM: f64 = 2000.0;

const MIN_PHASES: usize = 128;

const HALF_SPAN_LOW_RATE: usize = 20;

const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// See docs/adr/0006-resampling.md#polyphase-resampler-design.
#[derive(Debug)]
pub struct PolyphaseResampler {
    up: usize,
    down: usize,
    phases: Vec<Vec<f32>>,
    num_phases: usize,
    nominal_step: f64,
    step: f64,
    buf: Vec<f32>,
    pos: f64,
    delay_output: usize,
}

impl PolyphaseResampler {
    /// See docs/adr/0006-resampling.md#untrusted-rate-bounds.
    ///
    /// # Panics
    ///
    /// Panics when either rate is zero.
    #[must_use]
    pub fn new(input_rate: u32, output_rate: u32, max_input_len: usize) -> Self {
        let (up, down) = Self::reduced_ratio(input_rate, output_rate);
        let oversampling = MIN_PHASES.div_ceil(up).max(1);
        let num_phases = up * oversampling;
        let step = down * oversampling;
        let widest = num_phases.max(step);
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

    /// The reduced `(up, down)` ratio [`PolyphaseResampler::new`] would
    /// build for `input_rate → output_rate`, without designing anything.
    ///
    /// # Panics
    ///
    /// Panics when either rate is zero.
    #[must_use]
    pub const fn reduced_ratio(input_rate: u32, output_rate: u32) -> (usize, usize) {
        assert!(input_rate > 0 && output_rate > 0);
        let divisor = gcd(input_rate as usize, output_rate as usize);
        (
            output_rate as usize / divisor,
            input_rate as usize / divisor,
        )
    }

    /// See docs/adr/0006-resampling.md#integer-group-delay.
    #[must_use]
    pub const fn delay_output_samples(&self) -> usize {
        self.delay_output
    }

    /// See docs/adr/0001-realtime-audio-rules.md#realtime-allocation-ban.
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
        nominal + 2
    }

    /// See docs/adr/0006-resampling.md#drift-correction-design.
    pub fn set_drift_ppm(&mut self, ppm: f64) {
        let ppm = if ppm.is_finite() {
            ppm.clamp(-MAX_DRIFT_PPM, MAX_DRIFT_PPM)
        } else {
            0.0
        };
        self.step = self.nominal_step / ppm.mul_add(1e-6, 1.0);
    }

    /// See docs/adr/0006-resampling.md#polyphase-streaming-contract.
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
        let keep_from = base.saturating_sub(hist_len).min(self.buf.len());
        self.buf.drain(..keep_from);
        self.pos -= (keep_from * self.num_phases).to_f64();
    }

    /// See docs/adr/0006-resampling.md#resampler-reset-semantics.
    pub fn reset(&mut self) {
        let hist_len = self.phases[0].len() - 1;
        self.buf.clear();
        self.buf.resize(hist_len, 0.0);
        self.pos = (hist_len * self.num_phases).to_f64();
    }

    fn locate(&self) -> (usize, f64) {
        let phases = self.num_phases.to_f64();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "pos is non-negative and bounded by buf.len() × num_phases (small)"
        )]
        let mut base = (self.pos / phases).floor() as usize;
        let mut phase_pos = base.to_f64().mul_add(-phases, self.pos);
        if phase_pos < 0.0 {
            base = base.saturating_sub(1);
            phase_pos += phases;
        } else if phase_pos >= phases {
            base += 1;
            phase_pos -= phases;
        }
        (base, phase_pos.clamp(0.0, phases.next_down()))
    }

    fn dot(&self, phase: usize, base: usize) -> f32 {
        let mut acc = 0.0_f32;
        for (m, coef) in self.phases[phase].iter().enumerate() {
            acc = coef.mul_add(self.buf[base - m], acc);
        }
        acc
    }
}

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
            sig_energy = reference.mul_add(reference, sig_energy);
        }
        let snr_db = 10.0 * (sig_energy / err_energy).log10();
        assert!(snr_db > 50.0, "SNR too low: {snr_db} dB");
    }

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

    fn sine_snr_db(output: &[f32], rate: u32, freq: f64, delay: usize, skip: usize) -> f64 {
        let mut err = 0.0_f64;
        let mut sig = 0.0_f64;
        for (n, out) in output.iter().enumerate().skip(skip) {
            #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
            let position = (n - delay) as f64;
            let reference =
                (2.0 * std::f64::consts::PI * freq * position / f64::from(rate)).sin() * 0.5;
            err += (f64::from(*out) - reference).powi(2);
            sig = reference.mul_add(reference, sig);
        }
        10.0 * (sig / err).log10()
    }

    fn rms(samples: &[f32]) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
        let len = samples.len() as f64;
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / len).sqrt()
    }

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
            if ratio.1 == 1 {
                assert_eq!(
                    resampler.delay_output_samples(),
                    (taps_for_factor(ratio.0) - 1) / 2,
                    "{rate} Hz"
                );
            }
        }
        assert_eq!(
            PolyphaseResampler::new(44_100, 48_000, 480).delay_output_samples(),
            22
        );
    }

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

    #[test]
    fn polyphase_upsampling_keeps_the_top_of_the_band() {
        let input = sine(44_100, 15_000.0, 44_100);
        let delay = PolyphaseResampler::new(44_100, 48_000, 480).delay_output_samples();
        let output = convert(44_100, 48_000, &input, 480, 0.0);
        let snr = sine_snr_db(&output, 48_000, 15_000.0, delay, 2 * delay + 500);
        assert!(snr > 60.0, "15 kHz tone SNR too low: {snr} dB");
    }

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
                    #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
                    let position = n as f64 / stretch - delay as f64;
                    let reference =
                        (2.0 * std::f64::consts::PI * freq * position / 48_000.0).sin() * 0.5;
                    err += (f64::from(*out) - reference).powi(2);
                    sig = reference.mul_add(reference, sig);
                }
                let snr = 10.0 * (sig / err).log10();
                assert!(snr > 60.0, "{freq} Hz at {ppm} ppm: SNR too low: {snr} dB");
            }
        }
    }

    #[test]
    fn polyphase_chunked_streaming_matches_single_call_under_drift() {
        let input = sine(44_100, 700.0, 22_050);
        for ppm in [0.0_f64, 137.0] {
            let one = convert(44_100, 48_000, &input, input.len(), ppm);
            let chunked = convert(44_100, 48_000, &input, 97, ppm);
            assert_eq!(one, chunked, "{ppm} ppm");
        }
    }

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
