//! Native-rate capture support for non-48 kHz microphones (issue #7).
//!
//! Bluetooth headset microphones run on telephony profiles (HFP/SCO at
//! 8/16/24 kHz) and cannot be switched to the 48 kHz engine rate, so the
//! transport captures them at their native rate and converts to 48 kHz
//! on the inference worker. Two problems are solved here, both
//! platform-independently so they stay unit-tested on every CI target:
//!
//! 1. **Rate conversion** ([`InputResampler`]): the nominal ratio is an
//!    exact integer (48000 / 8000/12000/16000/24000), handled by the same
//!    polyphase [`Interpolator`] the model path uses. Arbitrary-ratio
//!    conversion (e.g. 44.1 kHz-family microphones) is deliberately out
//!    of scope: telephony profiles are the target, and every one of them
//!    is an integer division of 48 kHz.
//! 2. **Clock drift** ([`DriftServo`] + the micro-ratio stage inside
//!    [`InputResampler`]): with the microphone and the virtual output on
//!    separate AUHAL instances there is no Aggregate Device to absorb
//!    the clock split, so drift is compensated by adapting the effective
//!    conversion ratio a few hundred ppm around the integer factor,
//!    steered by the buffered-sample occupancy between the two clock
//!    domains (docs/tech-research.md §4.2, the DIY fallback).
//!
//! Everything is preallocated at construction time; `process` and
//! `update` perform no locking and no allocation in steady state (the
//! inference worker calls them, never the audio I/O callback —
//! docs/tech-research.md §9).
//!
//! Alignment note for the strength control: the dry/wet mixer taps its
//! dry signal from the *engine input*, i.e. after this conversion, so
//! the input resampler's delay shifts both paths equally and the
//! dry-compensation alignment (`crate::mix`) is unaffected by
//! construction. The `strength_alignment_survives_the_input_resampler`
//! test below proves it end to end.

use crate::error::StageError;
use crate::resample::Interpolator;
use crate::stage::ENGINE_SAMPLE_RATE;

/// Largest drift correction the servo may request, in parts per million.
///
/// ±2000 ppm (0.2%) is an order of magnitude above real crystal
/// mismatches, small enough to be inaudible (≈3.5 cents), and bounds how
/// fast the servo can slew the ratio while recovering a priming offset.
pub const MAX_DRIFT_PPM: f64 = 2000.0;

/// Catmull-Rom cubic interpolation between `p1` and `p2` (`mu` in
/// `0.0..1.0`). Exact at the knots: `mu == 0.0` returns `p1` bit-exactly.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, mu: f32) -> f32 {
    let a = 3.0_f32.mul_add(p1 - p2, p3 - p0);
    let b = 4.0_f32.mul_add(p2, 2.0_f32.mul_add(p0, -5.0_f32.mul_add(p1, p3)));
    let c = p2 - p0;
    (0.5 * mu).mul_add(mu.mul_add(mu.mul_add(a, b), c), p1)
}

/// Number of zero samples seeding [`MicroResampler::buf`] (the cubic's
/// look-behind at stream start).
const MICRO_SEED: usize = 2;

/// [`MICRO_SEED`] as the reader's starting position.
const MICRO_SEED_POS: f64 = 2.0;

/// Streaming fractional reader whose step hovers around 1.0: the
/// micro-ratio half of drift compensation.
///
/// Reads a continuous signal from its input stream by Catmull-Rom cubic
/// interpolation at positions advancing by `step = 1 / (1 + ppm·1e-6)`
/// per output sample. At zero correction it is an exact passthrough
/// (`output[k] == input[k]`); at a nonzero correction it stretches or
/// squeezes the stream by that ratio. It runs at the 48 kHz engine rate
/// — *after* the integer-factor interpolation — where telephony-profile
/// content sits far below Nyquist, so the cubic's passband error is
/// negligible for the signals this path carries.
#[derive(Debug)]
struct MicroResampler {
    /// History + current chunk. The first two slots seed the cubic's
    /// look-behind at stream start.
    buf: Vec<f32>,
    /// Read position in `buf` coordinates (always ≥ 1.0 so the cubic's
    /// `p0` tap exists).
    pos: f64,
    /// Input samples consumed per output sample.
    step: f64,
}

impl MicroResampler {
    fn new(max_input_len: usize) -> Self {
        let mut buf = Vec::with_capacity(MICRO_SEED + max_input_len);
        buf.extend([0.0; MICRO_SEED]);
        Self {
            buf,
            pos: MICRO_SEED_POS,
            step: 1.0,
        }
    }

    fn set_ppm(&mut self, ppm: f64) {
        let ppm = if ppm.is_finite() {
            ppm.clamp(-MAX_DRIFT_PPM, MAX_DRIFT_PPM)
        } else {
            0.0
        };
        self.step = 1.0 / ppm.mul_add(1e-6, 1.0);
    }

    /// Appends `input` and emits every output sample whose cubic support
    /// is complete (a two-sample look-ahead is held back until the next
    /// call; the signal itself is not delayed).
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        loop {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "pos is kept in 0..buf.len() (a few thousand) by the drain below"
            )]
            let index = self.pos.floor() as usize;
            if index + 2 >= self.buf.len() {
                break;
            }
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                reason = "the fraction is in 0..1 and buffer indices are tiny"
            )]
            let mu = (self.pos - index as f64) as f32;
            output.push(catmull_rom(
                self.buf[index - 1],
                self.buf[index],
                self.buf[index + 1],
                self.buf[index + 2],
                mu,
            ));
            self.pos += self.step;
        }
        // Drop the consumed prefix, keeping the cubic's look-behind.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "pos is bounded by buf.len() (a few thousand)"
        )]
        let keep_from = (self.pos.floor() as usize).saturating_sub(1);
        self.buf.drain(..keep_from);
        #[expect(
            clippy::cast_precision_loss,
            reason = "keep_from is bounded by buf.len() (a few thousand)"
        )]
        {
            self.pos -= keep_from as f64;
        }
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.buf.extend([0.0; MICRO_SEED]);
        self.pos = MICRO_SEED_POS;
    }
}

/// Streaming converter from a microphone's native rate to the 48 kHz
/// engine rate.
///
/// Integer-factor polyphase interpolation (the quality-critical
/// anti-imaging filter, shared with the model path) followed by the
/// micro-ratio drift stage.
#[derive(Debug)]
pub struct InputResampler {
    interpolator: Interpolator,
    micro: MicroResampler,
    factor: usize,
    /// Interpolator output, staged between the two stages. Preallocated.
    stage_buf: Vec<f32>,
}

impl InputResampler {
    /// Creates a converter from `native_rate` Hz to the engine rate,
    /// preallocating for input chunks of up to `max_input_len` native
    /// samples per [`InputResampler::process`] call.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Unsupported`] when `native_rate` is not a
    /// proper integer divisor of [`ENGINE_SAMPLE_RATE`] (arbitrary-ratio
    /// conversion is out of scope; see the module docs).
    pub fn new(native_rate: u32, max_input_len: usize) -> Result<Self, StageError> {
        if native_rate == 0
            || native_rate >= ENGINE_SAMPLE_RATE
            || !ENGINE_SAMPLE_RATE.is_multiple_of(native_rate)
        {
            return Err(StageError::Unsupported(format!(
                "native rate {native_rate} Hz is not an integer divisor of the \
                 {ENGINE_SAMPLE_RATE} Hz engine rate"
            )));
        }
        let factor = (ENGINE_SAMPLE_RATE / native_rate) as usize;
        let stage_capacity = factor * max_input_len;
        Ok(Self {
            interpolator: Interpolator::new(factor, max_input_len),
            micro: MicroResampler::new(stage_capacity),
            factor,
            stage_buf: Vec::with_capacity(stage_capacity),
        })
    }

    /// Integer upsampling factor (`ENGINE_SAMPLE_RATE / native_rate`).
    #[must_use]
    pub const fn factor(&self) -> usize {
        self.factor
    }

    /// Group delay in samples at the 48 kHz output rate. The micro stage
    /// adds none (its look-ahead is buffering, not signal delay), so this
    /// is the interpolator's filter delay.
    #[must_use]
    pub const fn delay_output_samples(&self) -> usize {
        self.interpolator.delay_output_samples()
    }

    /// Applies a drift correction in parts per million (clamped to
    /// ±[`MAX_DRIFT_PPM`]; non-finite values read as zero). Positive
    /// values stretch the stream — more output samples per input sample —
    /// which raises the buffered occupancy downstream.
    pub fn set_drift_ppm(&mut self, ppm: f64) {
        self.micro.set_ppm(ppm);
    }

    /// Converts one chunk of native-rate samples, appending 48 kHz
    /// samples to `output` (roughly `input.len() × factor`, modulated by
    /// the drift correction). Allocation-free while `input` stays within
    /// the preallocated `max_input_len`.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.stage_buf.clear();
        self.interpolator.process(input, &mut self.stage_buf);
        self.micro.process(&self.stage_buf, output);
    }

    /// Clears filter and reader history (the drift correction persists).
    pub fn reset(&mut self) {
        self.interpolator.reset();
        self.micro.reset();
        self.stage_buf.clear();
    }
}

/// First-order correction horizon in samples at the engine rate: an
/// occupancy error is corrected with a time constant of
/// `HORIZON / 48000` ≈ 10 s — far slower than Bluetooth burst jitter,
/// fast enough to absorb worst-case drift long before the rings fill.
const SERVO_HORIZON_SAMPLES: f64 = 480_000.0;

/// EMA coefficient applied per [`DriftServo::update`] call (one call per
/// 10 ms engine block → ≈0.5 s smoothing window), filtering Bluetooth
/// burst-delivery noise out of the occupancy signal.
const SERVO_SMOOTHING: f64 = 0.02;

/// Ring-occupancy servo steering the drift correction: the control half
/// of the DIY clock-drift compensation (docs/tech-research.md §4.2).
///
/// The capture side produces samples on the microphone's clock; the
/// output side consumes them on the virtual output's clock. Any rate
/// mismatch shows up as a trend in the number of samples buffered
/// between the two, so the servo smooths that occupancy and requests a
/// correction proportional to its distance from the target:
///
/// - occupancy above target → the microphone clock runs fast → squeeze
///   (negative ppm, fewer output samples per input sample);
/// - occupancy below target → stretch (positive ppm).
///
/// Proportional control leaves a small steady-state offset (the residual
/// occupancy error that sustains the correction) — bounded by
/// `drift × horizon`, e.g. 48 samples (1 ms) at 100 ppm — which is
/// harmless and keeps the loop trivially stable.
#[derive(Debug)]
pub struct DriftServo {
    target: f64,
    smoothed: Option<f64>,
}

impl DriftServo {
    /// Creates a servo holding the buffered occupancy at
    /// `target_samples` (samples at the engine rate — normally the
    /// output ring's priming fill).
    #[must_use]
    pub const fn new(target_samples: usize) -> Self {
        #[expect(
            clippy::cast_precision_loss,
            reason = "occupancy targets are far below 2^53"
        )]
        let target = target_samples as f64;
        Self {
            target,
            smoothed: None,
        }
    }

    /// Feeds one occupancy observation (total samples buffered between
    /// the capture and output clock domains, in engine-rate samples;
    /// expected once per engine block) and returns the drift correction
    /// in ppm to apply via [`InputResampler::set_drift_ppm`].
    pub fn update(&mut self, occupancy_samples: usize) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "ring occupancies are far below 2^53"
        )]
        let observed = occupancy_samples as f64;
        let smoothed = self.smoothed.map_or(observed, |previous| {
            SERVO_SMOOTHING.mul_add(observed - previous, previous)
        });
        self.smoothed = Some(smoothed);
        ((self.target - smoothed) / SERVO_HORIZON_SAMPLES * 1e6)
            .clamp(-MAX_DRIFT_PPM, MAX_DRIFT_PPM)
    }

    /// Forgets the smoothed occupancy (for a rebuilt transport).
    pub const fn reset(&mut self) {
        self.smoothed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::IntensityControl;
    use crate::switch::SwitchingEngine;

    fn sine(rate: u32, freq: f32, len: usize) -> Vec<f32> {
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        (0..len)
            .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    /// Deterministic speech-like test signal: a gliding fundamental with
    /// harmonics and a syllabic amplitude envelope — broadband and
    /// aperiodic enough for an unambiguous cross-correlation peak,
    /// band-limited like telephony speech (content stays below ~1.3 kHz,
    /// under every telephony profile's Nyquist). Defined in continuous
    /// time, so two rates sample the same underlying waveform.
    fn pseudo_speech(rate: u32, len: usize) -> Vec<f32> {
        #[expect(clippy::cast_precision_loss, reason = "test signal indices are small")]
        (0..len)
            .map(|n| {
                let t = n as f32 / rate as f32;
                let f0 = 20.0_f32.mul_add((3.0 * t).sin(), 130.0);
                let phase = 2.0 * std::f32::consts::PI * f0 * t;
                let voiced: f32 = (1..=8_u32)
                    .map(|h| {
                        #[expect(clippy::cast_precision_loss, reason = "harmonic numbers are tiny")]
                        let harmonic = h as f32;
                        (phase * harmonic).sin() / harmonic
                    })
                    .sum();
                let envelope = 0.5_f32.mul_add((2.0 * std::f32::consts::PI * 2.5 * t).sin(), 0.5);
                voiced * envelope * 0.15
            })
            .collect()
    }

    /// Lag (in samples) of the peak normalized cross-correlation of `b`
    /// against `a` — `b` is `a` delayed by the returned lag — searched
    /// over `0..=max_lag`, with the peak's correlation value.
    fn best_lag(reference: &[f32], delayed: &[f32], max_lag: usize) -> (usize, f64) {
        let mut best = (0, f64::MIN);
        for lag in 0..=max_lag {
            let overlap = reference.len().min(delayed.len().saturating_sub(lag));
            let mut dot = 0.0_f64;
            let mut energy_ref = 0.0_f64;
            let mut energy_del = 0.0_f64;
            for index in 0..overlap {
                let sample_ref = f64::from(reference[index]);
                let sample_del = f64::from(delayed[index + lag]);
                dot = sample_ref.mul_add(sample_del, dot);
                energy_ref = sample_ref.mul_add(sample_ref, energy_ref);
                energy_del = sample_del.mul_add(sample_del, energy_del);
            }
            let denom = (energy_ref * energy_del).sqrt();
            let corr = if denom > 0.0 { dot / denom } else { 0.0 };
            if corr > best.1 {
                best = (lag, corr);
            }
        }
        best
    }

    #[test]
    fn micro_resampler_at_unity_is_bit_exact_passthrough() {
        let input = sine(48_000, 1000.0, 4800);
        let mut micro = MicroResampler::new(input.len());
        let mut output = Vec::new();
        micro.process(&input, &mut output);
        // The two-sample cubic look-ahead is held back, not delayed.
        assert_eq!(output.len(), input.len() - MICRO_SEED);
        assert_eq!(output, input[..output.len()]);
    }

    #[test]
    fn micro_resampler_chunked_matches_single_call() {
        let input = sine(48_000, 700.0, 4800);
        let mut one = Vec::new();
        MicroResampler::new(input.len()).process(&input, &mut one);

        let mut chunked = Vec::new();
        let mut micro = MicroResampler::new(97);
        for chunk in input.chunks(97) {
            micro.process(chunk, &mut chunked);
        }
        assert_eq!(one, chunked);
    }

    /// A fixed correction must time-warp the signal by exactly that
    /// ratio: the output count matches, and the waveform tracks the
    /// analytically warped sine with high fidelity.
    #[test]
    fn micro_resampler_tracks_a_fixed_drift_correction() {
        let rate = 48_000_u32;
        let freq = 1000.0_f32;
        let input = sine(rate, freq, rate as usize * 2);
        for ppm in [-500.0_f64, 500.0] {
            let mut micro = MicroResampler::new(1024);
            micro.set_ppm(ppm);
            let mut output = Vec::new();
            for chunk in input.chunks(1024) {
                micro.process(chunk, &mut output);
            }
            let step = 1.0 / ppm.mul_add(1e-6, 1.0);
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "test sample counts are small and positive"
            )]
            let expected_len = ((input.len() - MICRO_SEED) as f64 / step) as usize;
            assert!(
                output.len().abs_diff(expected_len) <= 2,
                "{ppm} ppm: {} outputs, expected ≈{expected_len}",
                output.len()
            );
            let mut err = 0.0_f64;
            let mut sig = 0.0_f64;
            for (k, out) in output.iter().enumerate() {
                #[expect(clippy::cast_precision_loss, reason = "test sample counts are small")]
                let position = k as f64 * step;
                let reference = (2.0 * std::f64::consts::PI * f64::from(freq) * position
                    / f64::from(rate))
                .sin()
                    * 0.5;
                err += (f64::from(*out) - reference).powi(2);
                sig += reference * reference;
            }
            let snr_db = 10.0 * (sig / err).log10();
            assert!(snr_db > 60.0, "{ppm} ppm: SNR too low: {snr_db} dB");
        }
    }

    /// Every telephony-profile factor (24 kHz → ×2, 16 kHz → ×3,
    /// 12 kHz → ×4, 8 kHz → ×6) must reproduce speech-like audio delayed
    /// by exactly the reported group delay: the cross-correlation peak
    /// sits at `delay_output_samples()` with near-unity correlation.
    #[test]
    fn input_resampler_reports_its_exact_delay_for_every_integer_ratio() {
        for native in [24_000_u32, 16_000, 12_000, 8_000] {
            let mut resampler = InputResampler::new(native, 480).expect("integer ratio");
            assert_eq!(resampler.factor(), (48_000 / native) as usize);
            let captured = pseudo_speech(native, native as usize);
            let mut output = Vec::new();
            for chunk in captured.chunks(480) {
                resampler.process(chunk, &mut output);
            }
            let delay = resampler.delay_output_samples();
            let reference = pseudo_speech(48_000, output.len());
            let (lag, corr) = best_lag(&reference, &output, 2 * delay + 500);
            assert_eq!(
                lag, delay,
                "{native} Hz: cross-correlation peak off the reported delay"
            );
            assert!(corr > 0.999, "{native} Hz: correlation too low: {corr}");
        }
    }

    #[test]
    fn input_resampler_rejects_non_integer_ratios() {
        for rate in [0_u32, 44_100, 32_000, 48_000, 96_000] {
            assert!(
                InputResampler::new(rate, 480).is_err(),
                "{rate} Hz must be rejected"
            );
        }
    }

    #[test]
    fn input_resampler_chunked_matches_single_call() {
        let input = sine(16_000, 440.0, 4800);
        let mut one = Vec::new();
        InputResampler::new(16_000, input.len())
            .expect("integer ratio")
            .process(&input, &mut one);

        let mut chunked = Vec::new();
        let mut resampler = InputResampler::new(16_000, 160).expect("integer ratio");
        for chunk in input.chunks(160) {
            resampler.process(chunk, &mut chunked);
        }
        assert_eq!(one, chunked);
    }

    /// Closed loop: a producer whose clock drifts against the consumer
    /// must settle near the target occupancy with the correction
    /// canceling the drift — no ring runaway over a simulated half hour.
    #[test]
    fn servo_cancels_clock_drift_in_a_simulated_session() {
        for drift_ppm in [-150.0_f64, 0.0, 150.0] {
            let target = 2400.0_f64;
            let mut servo = DriftServo::new(2400);
            let mut occupancy = target;
            let mut correction = 0.0_f64;
            let mut worst_after_settling = 0.0_f64;
            // 30 minutes of 10 ms blocks.
            let blocks = 30 * 60 * 100;
            for block in 0..blocks {
                // The capture clock delivers (1 + drift) × nominal; the
                // conversion applies (1 + correction); the output clock
                // consumes exactly 480 samples per block.
                occupancy = 480.0_f64.mul_add(
                    drift_ppm
                        .mul_add(1e-6, 1.0)
                        .mul_add(correction.mul_add(1e-6, 1.0), -1.0),
                    occupancy,
                );
                assert!(
                    occupancy > 0.0 && occupancy < 48_000.0,
                    "{drift_ppm} ppm: ring ran away at block {block}: {occupancy}"
                );
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "occupancy is kept positive and small by the assert above"
                )]
                let observed = occupancy.round() as usize;
                correction = servo.update(observed);
                if block > blocks / 2 {
                    worst_after_settling = worst_after_settling.max((occupancy - target).abs());
                }
            }
            // Settled: the correction cancels the drift and the occupancy
            // sits at the proportional controller's small residual.
            assert!(
                (correction + drift_ppm).abs() < 10.0,
                "{drift_ppm} ppm: correction settled at {correction}"
            );
            assert!(
                worst_after_settling < 200.0,
                "{drift_ppm} ppm: occupancy wandered {worst_after_settling} samples off target"
            );
        }
    }

    #[test]
    fn servo_clamps_extreme_corrections() {
        let mut servo = DriftServo::new(2400);
        // A wildly overfull chain must still request a bounded squeeze.
        let mut correction = 0.0;
        for _ in 0..10_000 {
            correction = servo.update(40_000);
        }
        assert!((correction + MAX_DRIFT_PPM).abs() < f64::EPSILON);
    }

    /// End-to-end strength alignment (the PR #15 C5 measurement, applied
    /// to the native-capture path): speech-like audio captured at 16 kHz,
    /// converted by the input resampler, and processed at 50% intensity
    /// through a silent stage with reported latency must come out as
    /// exactly half the delayed dry signal — cross-correlation measures
    /// zero residual offset, proving the input resampler's delay cannot
    /// disturb the dry-compensation alignment (both taps sit after it).
    #[test]
    fn strength_alignment_survives_the_input_resampler() {
        /// Silent stage with a reported latency, standing in for a model:
        /// at 50% intensity the engine output is exactly the half-gain,
        /// delay-compensated dry path.
        #[derive(Debug)]
        struct Silent {
            latency: usize,
        }
        impl crate::stage::Stage for Silent {
            fn id(&self) -> &'static str {
                "silent"
            }
            fn process_block(
                &mut self,
                input: &[f32],
                output: &mut [f32],
            ) -> Result<(), StageError> {
                if input.len() != output.len() {
                    return Err(StageError::BufferLen {
                        expected: input.len(),
                        got: output.len(),
                    });
                }
                output.fill(0.0);
                Ok(())
            }
            fn latency_samples(&self) -> usize {
                self.latency
            }
            fn reset(&mut self) {}
        }

        let native_rate = 16_000_u32;
        let captured = pseudo_speech(native_rate, native_rate as usize * 2);
        let mut resampler = InputResampler::new(native_rate, 160).expect("integer ratio");

        let stage_latency = 1234;
        let (_publisher, mut engine) = SwitchingEngine::new(
            Box::new(Silent {
                latency: stage_latency,
            }),
            240,
            480,
            IntensityControl::new(0.5),
        )
        .expect("engine builds");

        // Native chunks → 48 kHz engine blocks, mirroring the worker.
        let mut engine_input = Vec::new();
        for chunk in captured.chunks(160) {
            resampler.process(chunk, &mut engine_input);
        }
        let mut engine_output = vec![0.0_f32; engine_input.len()];
        for (block_in, block_out) in engine_input.chunks(480).zip(engine_output.chunks_mut(480)) {
            engine
                .process_block(block_in, block_out)
                .expect("engine block");
        }

        // The output must be the dry engine input, delayed by the
        // stage's reported latency and halved — sample-exact...
        for n in stage_latency + 500..engine_output.len() {
            let expected = engine_input[n - stage_latency] * 0.5;
            assert!(
                (engine_output[n] - expected).abs() < 1e-5,
                "sample {n}: {} != {expected}",
                engine_output[n]
            );
        }
        // ...and the cross-correlation measurement (the PR #15 method)
        // must find the peak at exactly the reported latency: zero
        // residual offset through the resampler.
        let (lag, corr) = best_lag(&engine_input, &engine_output, 4800);
        assert_eq!(lag, stage_latency, "residual dry/wet misalignment");
        assert!(corr > 0.999, "correlation too low: {corr}");
    }
}
