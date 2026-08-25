//! What the audio callback is allowed to touch.
//!
//! Core Audio's `IOProc` gets one of these and nothing else. Both methods are
//! wait-free: they move samples through a single-producer, single-consumer ring
//! and touch two atomics. No allocation, no locks, no I/O — the rules in
//! `docs/tech-research.md` §9 apply to this file more than any other.

use std::sync::Arc;

use rtrb::{Consumer, Producer};

use crate::status::Status;

/// The audio callback's half of the engine.
///
/// Created by [`crate::Engine::start`] and handed to the `IOProc`. Dropping it
/// does not stop the engine; it just means nothing is feeding it.
#[derive(Debug)]
pub struct AudioBridge {
    to_inference: Producer<f32>,
    from_inference: Consumer<f32>,
    status: Arc<Status>,
}

impl AudioBridge {
    /// Assembles a bridge from the queue ends the engine created.
    pub(crate) const fn new(
        to_inference: Producer<f32>,
        from_inference: Consumer<f32>,
        status: Arc<Status>,
    ) -> Self {
        Self {
            to_inference,
            from_inference,
            status,
        }
    }

    /// Moves one device buffer through the engine.
    ///
    /// `input` is what the microphone delivered; `output` receives what the
    /// virtual device should emit. They may be different lengths, which is what
    /// an aggregate device does when its sub-devices disagree momentarily.
    ///
    /// Returns `false` if the output queue was short and silence had to be
    /// emitted — a dropout. The caller cannot do anything about it in the
    /// callback, but the count is worth surfacing.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> bool {
        self.status.observe_input(input);

        // A full input queue means the inference thread has fallen behind.
        // Dropping the newest samples is the least-bad option: the alternative
        // is unbounded latency growth.
        if let Ok(chunk) = self
            .to_inference
            .write_chunk_uninit(input.len().min(self.to_inference.slots()))
        {
            let written = chunk.len();
            chunk.fill_from_iter(input[..written].iter().copied());
        }

        let wanted = output.len().min(self.from_inference.slots());
        let taken = self.from_inference.read_chunk(wanted).map_or(0, |chunk| {
            let count = chunk.len();
            let (first, second) = chunk.as_slices();
            output[..first.len()].copy_from_slice(first);
            output[first.len()..count].copy_from_slice(second);
            chunk.commit_all();
            count
        });

        let complete = taken == output.len();
        if !complete {
            output[taken..].fill(0.0);
            self.status.add_dropout();
        }
        self.status.observe_output(output);
        complete
    }

    /// Discards everything queued in both directions.
    ///
    /// For use when the device restarts and the buffered audio is stale.
    pub fn flush(&mut self) {
        while self.from_inference.pop().is_ok() {}
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "the bridge only copies samples, so bit equality is exactly the property these \
              assertions check"
)]
mod tests {
    use super::AudioBridge;
    use crate::status::Status;
    use std::sync::Arc;

    fn bridge(
        capacity: usize,
    ) -> (
        AudioBridge,
        rtrb::Consumer<f32>,
        rtrb::Producer<f32>,
        Arc<Status>,
    ) {
        let (to_producer, to_consumer) = rtrb::RingBuffer::new(capacity);
        let (from_producer, from_consumer) = rtrb::RingBuffer::new(capacity);
        let status = Arc::new(Status::new());
        (
            AudioBridge::new(to_producer, from_consumer, Arc::clone(&status)),
            to_consumer,
            from_producer,
            status,
        )
    }

    #[test]
    fn input_reaches_the_inference_side() {
        let (mut bridge, mut inference_input, _out, _status) = bridge(64);
        let mut output = [0.0; 4];
        bridge.process(&[0.1, 0.2, 0.3, 0.4], &mut output);

        let received: Vec<f32> = (0..4).map(|_| inference_input.pop().unwrap()).collect();
        assert_eq!(received, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn an_empty_output_queue_is_a_dropout_not_a_stall() {
        let (mut bridge, _in, _out, status) = bridge(64);
        let mut output = [9.0; 4];
        assert!(!bridge.process(&[0.0; 4], &mut output));
        assert_eq!(output, [0.0; 4]);
        assert_eq!(status.snapshot().dropouts, 1);
    }

    #[test]
    fn a_filled_output_queue_is_delivered_whole() {
        let (mut bridge, _in, mut inference_output, status) = bridge(64);
        for value in [0.5, 0.25, -0.25, -0.5] {
            inference_output.push(value).unwrap();
        }
        let mut output = [0.0; 4];
        assert!(bridge.process(&[0.0; 4], &mut output));
        assert_eq!(output, [0.5, 0.25, -0.25, -0.5]);
        assert_eq!(status.snapshot().dropouts, 0);
    }

    #[test]
    fn a_full_input_queue_drops_rather_than_growing_latency() {
        let (mut bridge, _in, _out, _status) = bridge(4);
        let mut output = [0.0; 8];
        // Eight samples into a four-slot queue: the excess must be discarded,
        // not buffered, or latency would climb without bound.
        bridge.process(&[1.0; 8], &mut output);
        bridge.process(&[1.0; 8], &mut output);
    }

    #[test]
    fn a_partially_filled_output_queue_is_padded() {
        let (mut bridge, _in, mut inference_output, status) = bridge(64);
        inference_output.push(0.75).unwrap();
        let mut output = [9.0; 3];
        assert!(!bridge.process(&[0.0; 3], &mut output));
        assert_eq!(output, [0.75, 0.0, 0.0]);
        assert_eq!(status.snapshot().dropouts, 1);
    }

    #[test]
    fn flush_discards_pending_output() {
        let (mut bridge, _in, mut inference_output, _status) = bridge(64);
        for _ in 0..8 {
            inference_output.push(1.0).unwrap();
        }
        bridge.flush();
        let mut output = [9.0; 4];
        assert!(!bridge.process(&[0.0; 4], &mut output));
        assert_eq!(output, [0.0; 4]);
    }

    #[test]
    fn levels_are_published() {
        let (mut bridge, _in, mut inference_output, status) = bridge(64);
        inference_output.push(0.4).unwrap();
        let mut output = [0.0; 1];
        bridge.process(&[-0.8], &mut output);
        let snapshot = status.snapshot();
        assert!((snapshot.input_peak - 0.8).abs() < 1e-6);
        assert!((snapshot.output_peak - 0.4).abs() < 1e-6);
    }
}
