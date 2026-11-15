//! See docs/adr/0006-resampling.md.

use crate::error::StageError;
pub use crate::resample::MAX_DRIFT_PPM;
use crate::resample::PolyphaseResampler;
use crate::stage::ENGINE_SAMPLE_RATE;

/// See docs/adr/0015-device-session.md.
pub const MIN_NATIVE_RATE: u32 = 8_000;

/// See docs/adr/0015-device-session.md.
pub const MAX_NATIVE_RATE: u32 = 192_000;

/// See docs/adr/0015-device-session.md.
pub const MAX_RATIO_TERM: usize = 1_024;

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
    /// See docs/adr/0015-device-session.md.
    ///
    /// # Errors
    ///
    /// Returns [`StageError::Unsupported`] when `native_rate` lies
    /// outside [`MIN_NATIVE_RATE`] to [`MAX_NATIVE_RATE`] (inclusive), or
    /// when its reduced ratio to the engine rate has a term above
    /// [`MAX_RATIO_TERM`] (a rate no real device reports, which would
    /// otherwise design an enormous filter bank).
    pub fn new(native_rate: u32, max_input_len: usize) -> Result<Self, StageError> {
        if !(MIN_NATIVE_RATE..=MAX_NATIVE_RATE).contains(&native_rate) {
            return Err(StageError::Unsupported(format!(
                "native rate {native_rate} Hz is outside the {MIN_NATIVE_RATE}–{MAX_NATIVE_RATE} Hz \
                 range the capture resampler converts to the {ENGINE_SAMPLE_RATE} Hz engine rate"
            )));
        }
        let (up, down) = PolyphaseResampler::reduced_ratio(native_rate, ENGINE_SAMPLE_RATE);
        if up > MAX_RATIO_TERM || down > MAX_RATIO_TERM {
            return Err(StageError::Unsupported(format!(
                "native rate {native_rate} Hz reduces to {up}/{down} against the \
                 {ENGINE_SAMPLE_RATE} Hz engine rate; the capture resampler accepts ratio terms \
                 up to {MAX_RATIO_TERM} (the standard 8–192 kHz rate families)"
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

    /// See docs/adr/0006-resampling.md.
    #[must_use]
    pub const fn ratio(&self) -> (usize, usize) {
        self.resampler.ratio()
    }

    /// See docs/adr/0006-resampling.md.
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

    /// See docs/adr/0006-resampling.md.
    #[must_use]
    pub fn max_output_len(&self, input_len: usize) -> usize {
        self.resampler.max_output_len(input_len)
    }

    /// See docs/adr/0006-resampling.md.
    pub fn set_drift_ppm(&mut self, ppm: f64) {
        self.resampler.set_drift_ppm(ppm);
    }

    /// See docs/adr/0001-realtime-audio-rules.md.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.resampler.process(input, output);
    }

    /// See docs/adr/0006-resampling.md.
    pub fn reset(&mut self) {
        self.resampler.reset();
    }
}

const SERVO_HORIZON_SAMPLES: f64 = 480_000.0;

const SERVO_SMOOTHING: f64 = 0.02;

/// See docs/adr/0006-resampling.md.
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

    /// See docs/adr/0006-resampling.md.
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
        for rate in [16_001_u32, 44_099, 22_222] {
            let error = InputResampler::new(rate, 480).expect_err("must be rejected");
            let message = error.to_string();
            assert!(
                message.contains(&rate.to_string()) && message.contains("ratio"),
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

    #[test]
    fn servo_cancels_clock_drift_in_a_simulated_session() {
        for drift_ppm in [-150.0_f64, 0.0, 150.0] {
            let target = 2400.0_f64;
            let mut servo = DriftServo::new(2400);
            let mut occupancy = target;
            let mut correction = 0.0_f64;
            let mut worst_after_settling = 0.0_f64;
            let blocks = 30 * 60 * 100;
            for block in 0..blocks {
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
    fn servo_holds_occupancy_through_the_rational_resampler() {
        let target = 2400_usize;
        let blocks = 2 * 60 * 100;
        let burst_blocks = 3;
        for drift_ppm in [-200.0_f64, 150.0] {
            let mut resampler = InputResampler::new(44_100, 480).expect("supported rate");
            let mut servo = DriftServo::new(target);
            let mut output_ring = target;
            let mut native_ring: std::collections::VecDeque<f32> =
                std::collections::VecDeque::new();
            let mut pending: std::collections::VecDeque<f32> = std::collections::VecDeque::new();
            let mut converted = Vec::with_capacity(resampler.max_output_len(480));
            let mut native_chunk = Vec::with_capacity(480);
            let mut native_due = 0.0_f64;
            let mut phase = 0.0_f32;
            let mut underruns = 0_usize;
            let mut correction = 0.0_f64;
            let mut worst_after_settling = 0_usize;
            let mut residue_seen = false;
            for block in 0..blocks {
                if output_ring < 480 {
                    underruns += 1;
                    output_ring = 0;
                } else {
                    output_ring -= 480;
                }
                native_due = 441.0_f64.mul_add(drift_ppm.mul_add(1e-6, 1.0), native_due);
                if block % burst_blocks == 0 {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "native_due is a small positive count"
                    )]
                    let native_count = native_due.floor() as usize;
                    #[expect(clippy::cast_precision_loss, reason = "small count")]
                    {
                        native_due -= native_count as f64;
                    }
                    native_ring.extend((0..native_count).map(|_| {
                        phase = (phase + 1000.0 / 44_100.0).fract();
                        (2.0 * std::f32::consts::PI * phase).sin() * 0.5
                    }));
                }
                let take = native_ring.len().min(480);
                native_chunk.clear();
                native_chunk.extend(native_ring.drain(..take));
                converted.clear();
                resampler.process(&native_chunk, &mut converted);
                pending.extend(converted.iter().copied());
                while pending.len() >= 480 {
                    pending.drain(..480);
                    output_ring += 480;
                }
                let native_residue = resampler.to_engine_samples(native_ring.len());
                residue_seen |= native_residue > 0;
                let occupancy = output_ring + pending.len() + native_residue;
                assert!(
                    occupancy < 48_000,
                    "{drift_ppm} ppm: ring ran away at block {block}: {occupancy}"
                );
                correction = servo.update(occupancy);
                resampler.set_drift_ppm(correction);
                if block > blocks / 2 {
                    worst_after_settling = worst_after_settling.max(occupancy.abs_diff(target));
                }
            }
            assert!(
                residue_seen,
                "{drift_ppm} ppm: the native-ring term was never exercised"
            );
            assert_eq!(underruns, 0, "{drift_ppm} ppm: output ring ran dry");
            assert!(
                (correction + drift_ppm).abs() < 15.0,
                "{drift_ppm} ppm: correction settled at {correction}"
            );
            assert!(
                worst_after_settling < 700,
                "{drift_ppm} ppm: occupancy wandered {worst_after_settling} samples off target"
            );
        }
    }

    #[test]
    fn servo_clamps_extreme_corrections() {
        let mut servo = DriftServo::new(2400);
        let mut correction = 0.0;
        for _ in 0..10_000 {
            correction = servo.update(40_000);
        }
        assert!((correction + MAX_DRIFT_PPM).abs() < f64::EPSILON);
    }

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

        for n in stage_latency + 500..engine_output.len() {
            let expected = engine_input[n - stage_latency] * 0.5;
            assert!(
                (engine_output[n] - expected).abs() < 1e-5,
                "{native_rate} Hz, sample {n}: {} != {expected}",
                engine_output[n]
            );
        }
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
