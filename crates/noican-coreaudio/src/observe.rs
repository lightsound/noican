//! See docs/adr/0001-realtime-audio-rules.md#lock-free-worker-observability.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const LEVEL_DECAY_PER_BLOCK: f32 = 0.9;

/// See docs/adr/0001-realtime-audio-rules.md#lock-free-worker-observability.
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

/// See docs/adr/0001-realtime-audio-rules.md#lock-free-worker-observability.
pub const BLOCK_BUDGET_NS: u64 = 10_000_000;

/// See docs/adr/0001-realtime-audio-rules.md#lock-free-worker-observability.
#[derive(Debug, Default)]
pub struct WorkerBlockStats {
    blocks: AtomicU64,
    over_budget: AtomicU64,
    max_ns: AtomicU64,
}

impl WorkerBlockStats {
    /// Creates zeroed statistics.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            blocks: AtomicU64::new(0),
            over_budget: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
        }
    }

    /// Folds one block's processing time in (worker thread, relaxed
    /// atomics only).
    pub fn record(&self, elapsed_ns: u64) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        if elapsed_ns > BLOCK_BUDGET_NS {
            self.over_budget.fetch_add(1, Ordering::Relaxed);
        }
        self.max_ns.fetch_max(elapsed_ns, Ordering::Relaxed);
    }

    /// Zeroes every counter (worker start/exit, or per-model
    /// re-attribution from the control plane).
    pub fn reset(&self) {
        self.blocks.store(0, Ordering::Relaxed);
        self.over_budget.store(0, Ordering::Relaxed);
        self.max_ns.store(0, Ordering::Relaxed);
    }

    /// Blocks processed since the last reset.
    #[must_use]
    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Relaxed)
    }

    /// Blocks that exceeded [`BLOCK_BUDGET_NS`] since the last reset.
    #[must_use]
    pub fn over_budget(&self) -> u64 {
        self.over_budget.load(Ordering::Relaxed)
    }

    /// Longest single block since the last reset, in nanoseconds.
    #[must_use]
    pub fn max_ns(&self) -> u64 {
        self.max_ns.load(Ordering::Relaxed)
    }
}

fn fold_peak(bits: &AtomicU32, block: &[f32]) {
    let peak = block
        .iter()
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()))
        .min(1.0);
    let decayed = f32::from_bits(bits.load(Ordering::Relaxed)) * LEVEL_DECAY_PER_BLOCK;
    bits.store(peak.max(decayed).to_bits(), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn block_stats_count_blocks_budget_violations_and_the_maximum() {
        let stats = WorkerBlockStats::new();
        assert_eq!(stats.blocks(), 0);
        assert_eq!(stats.over_budget(), 0);
        assert_eq!(stats.max_ns(), 0);

        stats.record(1_000_000);
        stats.record(BLOCK_BUDGET_NS);
        stats.record(BLOCK_BUDGET_NS + 1);
        stats.record(21_500_000);
        stats.record(2_000_000);

        assert_eq!(stats.blocks(), 5);
        assert_eq!(stats.over_budget(), 2);
        assert_eq!(stats.max_ns(), 21_500_000);
    }

    #[test]
    fn block_stats_reset_zeroes_every_counter() {
        let stats = WorkerBlockStats::new();
        stats.record(15_000_000);
        stats.reset();
        assert_eq!(stats.blocks(), 0);
        assert_eq!(stats.over_budget(), 0);
        assert_eq!(stats.max_ns(), 0);
    }
}
