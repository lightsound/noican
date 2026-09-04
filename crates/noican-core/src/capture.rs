//! Native-rate capture support for non-48 kHz microphones (issue #7).
//!
//! Microphones that cannot run at the 48 kHz engine rate — Bluetooth
//! telephony profiles (HFP/SCO at 8/16/24 kHz), 44.1 kHz-family devices
//! (44.1/22.05/11.025 kHz), high-rate-only interfaces (88.2/96 kHz) —
//! are captured at their native rate and converted to 48 kHz on the
//! inference worker. Two problems are solved here, both
//! platform-independently so they stay unit-tested on every CI target:
//!
//! 1. **Rate conversion** ([`InputResampler`]): any native rate within
//!    [`MIN_NATIVE_RATE`] to [`MAX_NATIVE_RATE`] (inclusive) is converted by the
//!    exact reduced ratio through one [`PolyphaseResampler`] — 160/147
//!    for 44.1 kHz, 3/1 for 16 kHz, 1/2 for 96 kHz. (Earlier revisions
//!    accepted integer divisors of 48 kHz only; the rational polyphase
//!    design removed that limit without adding a stage or a dependency —
//!    see the type's docs for the alternatives weighed.)
//! 2. **Clock drift** ([`DriftServo`] + the fractional phase step of the
//!    same resampler): with the microphone and the virtual output on
//!    separate AUHAL instances there is no Aggregate Device to absorb
//!    the clock split, so drift is compensated by adapting the effective
//!    conversion ratio a few hundred ppm around the nominal ratio,
//!    steered by the buffered-sample occupancy between the two clock
//!    domains (docs/tech-research.md §4.2, the DIY fallback). The servo
//!    works in engine-rate samples and is ratio-agnostic; the transport
//!    converts its native-ring occupancy through
//!    [`InputResampler::to_engine_samples`].
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
pub use crate::resample::MAX_DRIFT_PPM;
use crate::resample::PolyphaseResampler;
use crate::stage::ENGINE_SAMPLE_RATE;

/// Lowest native capture rate the split transport accepts, in Hz: the
/// narrowest telephony profile (Bluetooth HFP narrow-band). Nothing
/// below it exists as a microphone format.
pub const MIN_NATIVE_RATE: u32 = 8_000;

/// Highest native capture rate the split transport accepts, in Hz.
///
/// The resampler handles any ratio; this bounds its filter memory and
/// the transport's per-pass work (a 192 kHz device delivers four native
/// samples per engine sample). Devices above 48 kHz normally also
/// advertise 48 kHz and take the aggregate path; the bound is for the
/// odd interface that does not.
pub const MAX_NATIVE_RATE: u32 = 192_000;

/// Streaming converter from a microphone's native rate to the 48 kHz
/// engine rate, with the drift correction folded in.
///
/// A thin, engine-rate-bound wrapper over [`PolyphaseResampler`]: it
/// fixes the output rate, validates the native rate range, and exposes
/// the ratio in the units the transport's drift servo needs.
#[derive(Debug)]
pub struct InputResampler {
    resampler: PolyphaseResampler,
    native_rate: u32,
}

impl InputResampler {
    /// Creates a converter from `native_rate` Hz to the engine rate,
    /// preallocating for input chunks of up to `max_input_len` native
    /// samples per [`InputResampler::process`] call.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Unsupported`] when `native_rate` lies
    /// outside [`MIN_NATIVE_RATE`] to [`MAX_NATIVE_RATE`] (inclusive).
    pub fn new(native_rate: u32, max_input_len: usize) -> Result<Self, StageError> {
        if !(MIN_NATIVE_RATE..=MAX_NATIVE_RATE).contains(&native_rate) {
            return Err(StageError::Unsupported(format!(
                "native rate {native_rate} Hz is outside the {MIN_NATIVE_RATE}–{MAX_NATIVE_RATE} Hz \
                 range the capture resampler converts to the {ENGINE_SAMPLE_RATE} Hz engine rate"
            )));
        }
        Ok(Self {
            resampler: PolyphaseResampler::new(native_rate, ENGINE_SAMPLE_RATE, max_input_len),
            native_rate,
        })
    }

    /// The native capture rate this converter was built for, in Hz.
    #[must_use]
    pub const fn native_rate(&self) -> u32 {
        self.native_rate
    }

    /// Reduced conversion ratio `(up, down)`: `up` engine samples per
    /// `down` native samples at zero drift (160/147 for 44.1 kHz, 3/1 for
    /// 16 kHz).
    #[must_use]
    pub const fn ratio(&self) -> (usize, usize) {
        self.resampler.ratio()
    }

    /// Converts a count of native-rate samples to the engine-rate samples
    /// they will become (rounded to nearest), so a transport can express
    /// its native-ring occupancy in the servo's units.
    #[must_use]
    pub const fn to_engine_samples(&self, native_samples: usize) -> usize {
        let (up, down) = self.resampler.ratio();
        (native_samples * up + down / 2) / down
    }

    /// Group delay in samples at the 48 kHz output rate — an integer by
    /// construction of the polyphase design.
    #[must_use]
    pub const fn delay_output_samples(&self) -> usize {
        self.resampler.delay_output_samples()
    }

    /// Upper bound on the samples one [`InputResampler::process`] call
    /// appends for `input_len` native samples (at the largest drift
    /// correction); reserve output buffers to it.
    #[must_use]
    pub fn max_output_len(&self, input_len: usize) -> usize {
        self.resampler.max_output_len(input_len)
    }

    /// Applies a drift correction in parts per million (clamped to
    /// ±[`MAX_DRIFT_PPM`]; non-finite values read as zero). Positive
    /// values stretch the stream — more output samples per input sample —
    /// which raises the buffered occupancy downstream.
    pub fn set_drift_ppm(&mut self, ppm: f64) {
        self.resampler.set_drift_ppm(ppm);
    }

    /// Converts one chunk of native-rate samples, appending 48 kHz
    /// samples to `output` (`input.len() × up / down` on average,
    /// modulated by the drift correction). Allocation-free while `input`
    /// stays within the preallocated `max_input_len` and `output` has
    /// room for [`InputResampler::max_output_len`].
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.resampler.process(input, output);
    }

    /// Clears filter history (the drift correction persists).
    pub fn reset(&mut self) {
        self.resampler.reset();
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

    /// Every capture rate — the telephony profiles (24 kHz → 2/1,
    /// 16 kHz → 3/1, 12 kHz → 4/1, 8 kHz → 6/1), the 44.1 kHz family
    /// (160/147, 320/147, 640/147), and the high rates (88.2 kHz →
    /// 80/147, 96 kHz → 1/2) — must reproduce speech-like audio delayed
    /// by exactly the reported group delay: the cross-correlation peak
    /// sits at `delay_output_samples()` with near-unity correlation.
    #[test]
    fn input_resampler_reports_its_exact_delay_for_every_rate() {
        let cases = [
            (24_000_u32, (2_usize, 1_usize)),
            (16_000, (3, 1)),
            (12_000, (4, 1)),
            (8_000, (6, 1)),
            (44_100, (160, 147)),
            (22_050, (320, 147)),
            (11_025, (640, 147)),
            (88_200, (80, 147)),
            (96_000, (1, 2)),
        ];
        for (native, ratio) in cases {
            let mut resampler = InputResampler::new(native, 480).expect("supported rate");
            assert_eq!(resampler.ratio(), ratio, "{native} Hz");
            assert_eq!(resampler.native_rate(), native);
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
    fn input_resampler_rejects_rates_outside_the_supported_range() {
        for rate in [0_u32, 7_999, 192_001, u32::MAX] {
            let error = InputResampler::new(rate, 480).expect_err("must be rejected");
            assert!(
                error.to_string().contains(&rate.to_string()),
                "unhelpful message for {rate} Hz: {error}"
            );
        }
        for rate in [MIN_NATIVE_RATE, 44_100, 48_000, MAX_NATIVE_RATE] {
            assert!(
                InputResampler::new(rate, 480).is_ok(),
                "{rate} Hz must be accepted"
            );
        }
    }

    /// The servo's occupancy conversion rounds to the nearest engine
    /// sample for every ratio shape.
    #[test]
    fn input_resampler_converts_native_counts_to_engine_samples() {
        let at_44k = InputResampler::new(44_100, 480).expect("supported rate");
        assert_eq!(at_44k.to_engine_samples(147), 160);
        assert_eq!(at_44k.to_engine_samples(441), 480);
        assert_eq!(at_44k.to_engine_samples(0), 0);
        let at_16k = InputResampler::new(16_000, 480).expect("supported rate");
        assert_eq!(at_16k.to_engine_samples(160), 480);
        let at_96k = InputResampler::new(96_000, 480).expect("supported rate");
        assert_eq!(at_96k.to_engine_samples(961), 481);
    }

    #[test]
    fn input_resampler_chunked_matches_single_call() {
        for (native, chunk) in [(16_000_u32, 160_usize), (44_100, 97)] {
            let input = sine(native, 440.0, 4800);
            let mut one = Vec::new();
            InputResampler::new(native, input.len())
                .expect("supported rate")
                .process(&input, &mut one);

            let mut chunked = Vec::new();
            let mut resampler = InputResampler::new(native, chunk).expect("supported rate");
            for piece in input.chunks(chunk) {
                resampler.process(piece, &mut chunked);
            }
            assert_eq!(one, chunked, "{native} Hz");
        }
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
        fn process_block(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), StageError> {
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

    /// End-to-end strength alignment (the PR #15 C5 measurement, applied
    /// to the native-capture path): speech-like audio captured at 16 kHz
    /// and at 44.1 kHz (an integer and a rational ratio), converted by
    /// the input resampler, and processed at 50% intensity
    /// through a silent stage with reported latency must come out as
    /// exactly half the delayed dry signal — cross-correlation measures
    /// zero residual offset, proving the input resampler's delay cannot
    /// disturb the dry-compensation alignment (both taps sit after it).
    #[test]
    fn strength_alignment_survives_the_input_resampler() {
        for native_rate in [16_000_u32, 44_100] {
            check_alignment(native_rate);
        }
    }

    fn check_alignment(native_rate: u32) {
        let captured = pseudo_speech(native_rate, native_rate as usize * 2);
        let mut resampler = InputResampler::new(native_rate, 160).expect("supported rate");

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
                "{native_rate} Hz, sample {n}: {} != {expected}",
                engine_output[n]
            );
        }
        // ...and the cross-correlation measurement (the PR #15 method)
        // must find the peak at exactly the reported latency: zero
        // residual offset through the resampler.
        let (lag, corr) = best_lag(&engine_input, &engine_output, 4800);
        assert_eq!(
            lag, stage_latency,
            "{native_rate} Hz: residual dry/wet misalignment"
        );
        assert!(
            corr > 0.999,
            "{native_rate} Hz: correlation too low: {corr}"
        );
    }
}
