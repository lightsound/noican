//! Lock-free observation of the stream flowing through the inference
//! worker.
//!
//! The preview (self-monitor) tee lets the worker duplicate its processed
//! output into a second, preallocated ring, and [`StreamLevels`] publishes
//! per-block peak meters — both without ever touching the meeting-facing
//! path. Everything here runs on the inference worker between device
//! callbacks and obeys the same real-time rules as the callbacks
//! themselves (docs/tech-research.md §9): no allocation, no locks — the
//! meters and the enable flag are plain atomics and the tee writes into a
//! preallocated lock-free ring.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rtrb::Producer;

/// Per-block decay applied to the stored peak (simple exponential
/// ballistics on the engine side, so every reader sees the same motion).
/// At the 10 ms worker block size this is ≈ −0.9 dB per block: a
/// full-scale peak falls below 1% (−40 dB) in about 440 ms — fast enough
/// to track speech pauses, slow enough that a 20 Hz UI poll never misses
/// a transient entirely.
const LEVEL_DECAY_PER_BLOCK: f32 = 0.9;

/// Linear peak levels of the model input (pre-processing) and model
/// output (post-processing), shared lock-free between the inference
/// worker and the control plane.
///
/// Values are `f32` bit patterns in [`AtomicU32`]s; `Relaxed` ordering is
/// sufficient because each value is independently meaningful and a
/// slightly stale read is harmless for a meter.
#[derive(Debug, Default)]
pub struct StreamLevels {
    input_bits: AtomicU32,
    output_bits: AtomicU32,
}

impl StreamLevels {
    /// Creates a meter pair at silence.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            input_bits: AtomicU32::new(0),
            output_bits: AtomicU32::new(0),
        }
    }

    /// Folds one processed block into the meters: each stored level
    /// becomes the larger of the block's clamped peak and the decayed
    /// previous level. Real-time safe (two relaxed atomics per meter).
    pub fn update(&self, input_block: &[f32], output_block: &[f32]) {
        fold_peak(&self.input_bits, input_block);
        fold_peak(&self.output_bits, output_block);
    }

    /// Resets both meters to silence (worker start and stop).
    pub fn reset(&self) {
        self.input_bits.store(0, Ordering::Relaxed);
        self.output_bits.store(0, Ordering::Relaxed);
    }

    /// Decayed linear peak of the model input, in `0.0..=1.0`.
    #[must_use]
    pub fn input(&self) -> f32 {
        f32::from_bits(self.input_bits.load(Ordering::Relaxed))
    }

    /// Decayed linear peak of the model output, in `0.0..=1.0`.
    #[must_use]
    pub fn output(&self) -> f32 {
        f32::from_bits(self.output_bits.load(Ordering::Relaxed))
    }
}

fn fold_peak(bits: &AtomicU32, block: &[f32]) {
    // NaN-proof: `f32::max` keeps the accumulator when the operand is NaN,
    // so a corrupt sample cannot poison the meter.
    let peak = block
        .iter()
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()))
        .min(1.0);
    let decayed = f32::from_bits(bits.load(Ordering::Relaxed)) * LEVEL_DECAY_PER_BLOCK;
    bits.store(peak.max(decayed).to_bits(), Ordering::Relaxed);
}

/// Copies one processed block into the preview monitor ring when
/// monitoring is enabled, without blocking.
///
/// Ring overrun (a monitor device draining slower than the engine clock)
/// silently discards the overflowing samples: preview tolerates minor
/// artifacts and must never push back on the meeting-facing path. When
/// monitoring is disabled the ring is not touched at all. Returns whether
/// the block was teed, so the skipped branch is observable in tests.
pub fn tee_into_monitor(enabled: &AtomicBool, monitor: &mut Producer<f32>, block: &[f32]) -> bool {
    if !enabled.load(Ordering::Acquire) {
        return false;
    }
    for &sample in block {
        let _overrun_discards = monitor.push(sample);
    }
    true
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    #[test]
    fn tee_delivers_identical_samples_when_enabled() {
        let (mut producer, mut consumer) = RingBuffer::new(16);
        let enabled = AtomicBool::new(true);
        let block = [0.1_f32, -0.5, 0.25, 1.0];
        assert!(tee_into_monitor(&enabled, &mut producer, &block));
        for &expected in &block {
            let delivered = consumer.pop().expect("teed sample is present");
            assert!((delivered - expected).abs() < f32::EPSILON);
        }
        assert!(consumer.pop().is_err(), "no extra samples were teed");
    }

    #[test]
    fn tee_is_skipped_when_disabled() {
        let (mut producer, mut consumer) = RingBuffer::new(16);
        let enabled = AtomicBool::new(false);
        assert!(!tee_into_monitor(&enabled, &mut producer, &[0.5, -0.5]));
        assert!(consumer.pop().is_err(), "disabled tee must not write");
    }

    #[test]
    fn tee_overrun_discards_instead_of_blocking() {
        let (mut producer, mut consumer) = RingBuffer::new(2);
        let enabled = AtomicBool::new(true);
        let block = [1.0_f32, 2.0, 3.0, 4.0];
        assert!(tee_into_monitor(&enabled, &mut producer, &block));
        // The first two samples fit; the overflow is dropped, not queued.
        assert!((consumer.pop().expect("first sample") - 1.0).abs() < f32::EPSILON);
        assert!((consumer.pop().expect("second sample") - 2.0).abs() < f32::EPSILON);
        assert!(consumer.pop().is_err());
    }

    #[test]
    fn levels_report_the_known_peak_and_clamp_to_unity() {
        let levels = StreamLevels::new();
        levels.update(&[0.0, -0.5, 0.25], &[0.0, 2.0, -3.0]);
        assert!(
            (levels.input() - 0.5).abs() < f32::EPSILON,
            "peak of |−0.5|"
        );
        assert!(
            (levels.output() - 1.0).abs() < f32::EPSILON,
            "out-of-range peaks clamp to 1.0"
        );
    }

    #[test]
    fn silence_reads_zero_and_reset_clears() {
        let levels = StreamLevels::new();
        assert!(levels.input().abs() < f32::EPSILON);
        assert!(levels.output().abs() < f32::EPSILON);
        levels.update(&[0.0; 480], &[0.0; 480]);
        assert!(levels.input().abs() < f32::EPSILON, "silence stays at zero");
        levels.update(&[0.8], &[0.4]);
        levels.reset();
        assert!(levels.input().abs() < f32::EPSILON);
        assert!(levels.output().abs() < f32::EPSILON);
    }

    #[test]
    fn levels_decay_exponentially_over_silent_blocks() {
        let levels = StreamLevels::new();
        levels.update(&[1.0], &[1.0]);
        let mut previous = levels.input();
        for _ in 0..10 {
            levels.update(&[0.0], &[0.0]);
            let current = levels.input();
            assert!(current < previous, "levels fall monotonically");
            assert!(
                previous.mul_add(-LEVEL_DECAY_PER_BLOCK, current).abs() < 1e-6,
                "one decay step per block"
            );
            previous = current;
        }
        let expected = LEVEL_DECAY_PER_BLOCK.powi(10);
        assert!((previous - expected).abs() < 1e-4);
    }

    #[test]
    fn nan_samples_do_not_poison_the_meter() {
        let levels = StreamLevels::new();
        levels.update(&[f32::NAN, 0.25], &[f32::NAN]);
        assert!((levels.input() - 0.25).abs() < f32::EPSILON);
        assert!(levels.output().abs() < f32::EPSILON);
    }
}
