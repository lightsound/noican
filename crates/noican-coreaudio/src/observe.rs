//! Lock-free observation of the stream flowing through the inference
//! worker.
//!
//! The preview (self-monitor) tee lets the worker duplicate its processed
//! output into a second, preallocated ring without ever touching the
//! meeting-facing path. It runs on the inference worker between device
//! callbacks and obeys the same real-time rules as the callbacks
//! themselves (docs/tech-research.md §9): no allocation, no locks — the
//! enable flag is a plain atomic and the tee writes into a preallocated
//! lock-free ring.

use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::Producer;

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
}
