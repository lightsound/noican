//! Shared state the audio and inference threads publish for the UI to read.
//!
//! Everything here is an atomic, because the alternative — a mutex the UI takes
//! while the audio callback wants it — is exactly what
//! `docs/tech-research.md` §9 rules out. Readers may see values from slightly
//! different instants, which is fine for a status display and for meters.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// A consistent-enough view of the engine, for display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapshot {
    /// Whether the audio callback is delivering samples.
    pub running: bool,
    /// Whether the active model is bypassed.
    pub bypassed: bool,
    /// Times the audio callback found the output queue short and emitted
    /// silence. Any non-zero value is audible.
    pub dropouts: u64,
    /// Times the inference thread found less input than it needed.
    ///
    /// Expected to be non-zero: it simply means the thread polled before a
    /// whole block had arrived.
    pub idle_polls: u64,
    /// Peak input level since the last read, in `[0, 1]`.
    pub input_peak: f32,
    /// Peak output level since the last read, in `[0, 1]`.
    pub output_peak: f32,
    /// End-to-end delay of the active stage, in milliseconds.
    pub latency_ms: f32,
    /// Whether a switch ramp is in progress.
    pub switching: bool,
}

/// Atomic status shared between all three threads.
#[derive(Debug, Default)]
pub struct Status {
    running: AtomicBool,
    bypassed: AtomicBool,
    switching: AtomicBool,
    dropouts: AtomicU64,
    idle_polls: AtomicU64,
    input_peak: AtomicU32,
    output_peak: AtomicU32,
    latency_ms: AtomicU32,
}

impl Status {
    /// Creates a status block with everything cleared.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a consistent-enough view, resetting the peak meters.
    ///
    /// Peaks are read-and-clear so that a UI polling at its own rate sees the
    /// loudest sample since it last looked, rather than a decaying value it has
    /// to smooth itself.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            running: self.running.load(Ordering::Relaxed),
            bypassed: self.bypassed.load(Ordering::Relaxed),
            switching: self.switching.load(Ordering::Relaxed),
            dropouts: self.dropouts.load(Ordering::Relaxed),
            idle_polls: self.idle_polls.load(Ordering::Relaxed),
            input_peak: f32::from_bits(self.input_peak.swap(0, Ordering::Relaxed)),
            output_peak: f32::from_bits(self.output_peak.swap(0, Ordering::Relaxed)),
            latency_ms: f32::from_bits(self.latency_ms.load(Ordering::Relaxed)),
        }
    }

    /// Marks the engine running or stopped.
    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::Relaxed);
    }

    /// Whether the engine is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Sets whether the active model is bypassed.
    pub fn set_bypassed(&self, bypassed: bool) {
        self.bypassed.store(bypassed, Ordering::Relaxed);
    }

    /// Whether the active model is bypassed.
    pub fn is_bypassed(&self) -> bool {
        self.bypassed.load(Ordering::Relaxed)
    }

    /// Sets whether a switch ramp is in progress.
    pub fn set_switching(&self, switching: bool) {
        self.switching.store(switching, Ordering::Relaxed);
    }

    /// Records the active stage's end-to-end delay.
    pub fn set_latency_ms(&self, latency_ms: f32) {
        self.latency_ms
            .store(latency_ms.to_bits(), Ordering::Relaxed);
    }

    /// Counts one audio-callback dropout.
    pub fn add_dropout(&self) {
        self.dropouts.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts one inference-thread poll that found nothing to do.
    pub fn add_idle_poll(&self) {
        self.idle_polls.fetch_add(1, Ordering::Relaxed);
    }

    /// Raises the input peak meter if `samples` contains anything louder.
    pub fn observe_input(&self, samples: &[f32]) {
        raise(&self.input_peak, peak(samples));
    }

    /// Raises the output peak meter if `samples` contains anything louder.
    pub fn observe_output(&self, samples: &[f32]) {
        raise(&self.output_peak, peak(samples));
    }
}

/// Largest absolute sample value in `samples`.
fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0f32, |highest, sample| highest.max(sample.abs()))
}

/// Raises `slot` to `value` if `value` is larger.
///
/// Compare-and-swap rather than a plain store, so a slow reader still sees the
/// loudest sample rather than the most recent block's.
fn raise(slot: &AtomicU32, value: f32) {
    let mut current = slot.load(Ordering::Relaxed);
    loop {
        if f32::from_bits(current) >= value {
            return;
        }
        match slot.compare_exchange_weak(
            current,
            value.to_bits(),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(seen) => current = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Status;

    #[test]
    fn peaks_are_read_and_clear() {
        let status = Status::new();
        status.observe_input(&[0.1, -0.6, 0.3]);
        status.observe_output(&[0.2]);

        let first = status.snapshot();
        assert!((first.input_peak - 0.6).abs() < 1e-6);
        assert!((first.output_peak - 0.2).abs() < 1e-6);

        let second = status.snapshot();
        assert!(second.input_peak.abs() < 1e-9);
        assert!(second.output_peak.abs() < 1e-9);
    }

    #[test]
    fn peaks_keep_the_loudest_between_reads() {
        let status = Status::new();
        status.observe_input(&[0.9]);
        status.observe_input(&[0.1]);
        assert!((status.snapshot().input_peak - 0.9).abs() < 1e-6);
    }

    #[test]
    fn flags_and_counters_round_trip() {
        let status = Status::new();
        assert!(!status.is_running());
        status.set_running(true);
        status.set_bypassed(true);
        status.set_switching(true);
        status.set_latency_ms(21.5);
        status.add_dropout();
        status.add_dropout();
        status.add_idle_poll();

        let snapshot = status.snapshot();
        assert!(snapshot.running && snapshot.bypassed && snapshot.switching);
        assert_eq!(snapshot.dropouts, 2);
        assert_eq!(snapshot.idle_polls, 1);
        assert!((snapshot.latency_ms - 21.5).abs() < 1e-6);
        assert!(status.is_bypassed());
    }
}
