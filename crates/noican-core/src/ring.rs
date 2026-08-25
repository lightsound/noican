//! A fixed-capacity FIFO of samples.
//!
//! This is the single-threaded buffer used inside [`crate::StageRunner`] to
//! bridge mismatched block sizes. Cross-thread hand-off between the audio
//! callback and the inference thread uses a lock-free SPSC queue instead; see
//! `noican-engine`.

/// A fixed-capacity, allocation-free FIFO of `f32` samples.
///
/// Capacity is reserved on construction. Pushing more than [`Self::vacancy`]
/// samples, or popping more than [`Self::len`], is reported through the return
/// value rather than panicking, so a real-time caller can degrade gracefully.
#[derive(Debug)]
pub struct SampleQueue {
    buffer: Box<[f32]>,
    head: usize,
    len: usize,
}

impl SampleQueue {
    /// Creates an empty queue that can hold `capacity` samples.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity.max(1)].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    /// Number of samples currently queued.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the queue holds no samples.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Total number of samples the queue can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Number of samples that can still be pushed.
    #[must_use]
    pub const fn vacancy(&self) -> usize {
        self.buffer.len() - self.len
    }

    /// Discards all queued samples.
    pub const fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Appends `samples`, returning how many were actually stored.
    ///
    /// A short return value means the queue was full; the tail of `samples` was
    /// dropped.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let capacity = self.buffer.len();
        let count = samples.len().min(capacity - self.len);
        let tail = (self.head + self.len) % capacity;
        let first = count.min(capacity - tail);
        self.buffer[tail..tail + first].copy_from_slice(&samples[..first]);
        if first < count {
            self.buffer[..count - first].copy_from_slice(&samples[first..count]);
        }
        self.len += count;
        count
    }

    /// Appends `count` zeros, returning how many were actually stored.
    pub fn push_silence(&mut self, count: usize) -> usize {
        let capacity = self.buffer.len();
        let count = count.min(capacity - self.len);
        let tail = (self.head + self.len) % capacity;
        let first = count.min(capacity - tail);
        self.buffer[tail..tail + first].fill(0.0);
        if first < count {
            self.buffer[..count - first].fill(0.0);
        }
        self.len += count;
        count
    }

    /// Removes the oldest `out.len()` samples into `out`.
    ///
    /// Returns how many samples were written. A short return value means the
    /// queue held fewer samples than requested; the remainder of `out` is
    /// untouched.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        let capacity = self.buffer.len();
        let count = out.len().min(self.len);
        let first = count.min(capacity - self.head);
        out[..first].copy_from_slice(&self.buffer[self.head..self.head + first]);
        if first < count {
            out[first..count].copy_from_slice(&self.buffer[..count - first]);
        }
        self.head = (self.head + count) % capacity;
        self.len -= count;
        count
    }

    /// Discards the oldest `count` samples, returning how many were removed.
    pub const fn discard(&mut self, count: usize) -> usize {
        let count = if count < self.len { count } else { self.len };
        self.head = (self.head + count) % self.buffer.len();
        self.len -= count;
        count
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "the queue only ever copies samples, so bit equality is exactly the property these \
              assertions check"
)]
mod tests {
    use super::SampleQueue;

    #[test]
    fn push_and_pop_round_trip() {
        let mut queue = SampleQueue::new(8);
        assert_eq!(queue.push(&[1.0, 2.0, 3.0]), 3);
        assert_eq!(queue.len(), 3);

        let mut out = [0.0; 2];
        assert_eq!(queue.pop(&mut out), 2);
        assert_eq!(out, [1.0, 2.0]);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn wraps_around_the_backing_buffer() {
        let mut queue = SampleQueue::new(4);
        queue.push(&[1.0, 2.0, 3.0]);
        let mut out = [0.0; 3];
        queue.pop(&mut out);

        // head is now at index 3; this push has to wrap.
        assert_eq!(queue.push(&[4.0, 5.0, 6.0, 7.0]), 4);
        let mut out = [0.0; 4];
        assert_eq!(queue.pop(&mut out), 4);
        assert_eq!(out, [4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn reports_short_push_when_full() {
        let mut queue = SampleQueue::new(2);
        assert_eq!(queue.push(&[1.0, 2.0, 3.0]), 2);
        assert_eq!(queue.vacancy(), 0);
        assert_eq!(queue.push(&[4.0]), 0);
    }

    #[test]
    fn reports_short_pop_when_empty() {
        let mut queue = SampleQueue::new(4);
        queue.push(&[1.0]);
        let mut out = [9.0; 3];
        assert_eq!(queue.pop(&mut out), 1);
        assert_eq!(out, [1.0, 9.0, 9.0]);
        assert!(queue.is_empty());
    }

    #[test]
    fn silence_and_discard() {
        let mut queue = SampleQueue::new(4);
        assert_eq!(queue.push_silence(3), 3);
        assert_eq!(queue.discard(2), 2);
        assert_eq!(queue.len(), 1);
        queue.clear();
        assert_eq!(queue.capacity(), 4);
        assert!(queue.is_empty());
    }
}
