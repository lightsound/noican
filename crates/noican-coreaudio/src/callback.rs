//! See docs/adr/0001-realtime-audio-rules.md#realtime-allocation-ban.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer};

/// See docs/adr/0003-coreaudio-transport.md#realtime-callback-frame-cap.
pub const MAX_CALLBACK_FRAMES: usize = 4_096;

/// See docs/adr/0003-coreaudio-transport.md#realtime-callback-frame-cap.
#[must_use]
pub fn capture_byte_size(frames: usize, capacity: usize) -> Option<u32> {
    if frames > capacity {
        return None;
    }
    frames
        .checked_mul(size_of::<f32>())
        .and_then(|bytes| u32::try_from(bytes).ok())
}

/// How much of a render buffer a callback may write: the interleaved
/// channel count and the number of whole frames that fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderGeometry {
    /// Interleaved channels per frame (at least 1).
    pub channels: usize,
    /// Whole frames the callback writes: the frames requested, capped by
    /// what the buffer can hold.
    pub frames: usize,
}

impl RenderGeometry {
    /// Total `f32` samples the callback writes (`frames * channels`).
    #[must_use]
    pub const fn samples(self) -> usize {
        self.frames * self.channels
    }
}

/// See docs/adr/0003-coreaudio-transport.md#render-geometry-zero-channel.
#[must_use]
pub fn render_geometry(
    data_byte_size: u32,
    number_channels: u32,
    frame_count: u32,
) -> RenderGeometry {
    let available = usize::try_from(data_byte_size)
        .unwrap_or(0)
        .saturating_div(size_of::<f32>());
    let channels = usize::try_from(number_channels).unwrap_or(0).max(1);
    let frames = usize::try_from(frame_count)
        .unwrap_or(0)
        .min(available.saturating_div(channels));
    RenderGeometry { channels, frames }
}

/// See docs/adr/0004-aggregate-output-routing.md#dual-mono-rendering.
pub fn fill_dual_mono(
    samples: &mut [f32],
    channels: usize,
    mut next: impl FnMut() -> Option<f32>,
) -> bool {
    let mut delivered = false;
    for frame in samples.chunks_exact_mut(channels.max(1)) {
        let value = next().map_or(0.0, |value| {
            delivered = true;
            value
        });
        frame.fill(value);
    }
    delivered
}

/// See docs/adr/0003-coreaudio-transport.md#capture-ring-overrun-accounting.
#[derive(Debug)]
pub struct CaptureFeed {
    ring: Producer<f32>,
    overruns: Arc<AtomicU64>,
}

impl CaptureFeed {
    /// Wraps the producer end of the capture ring and the overrun counter
    /// the control plane reads.
    #[must_use]
    pub const fn new(ring: Producer<f32>, overruns: Arc<AtomicU64>) -> Self {
        Self { ring, overruns }
    }

    /// See docs/adr/0003-coreaudio-transport.md#capture-ring-overrun-accounting.
    pub fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            let sample = if sample.is_finite() { sample } else { 0.0 };
            if self.ring.push(sample).is_err() {
                self.overruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn lose(&self, frames: usize) {
        self.overruns
            .fetch_add(u64::try_from(frames).unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

#[cfg(any(target_os = "macos", test))]
const RENDER_FAILURE_TOLERANCE_MS: usize = 100;

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
pub(crate) struct RenderFailures {
    run: usize,
    cycles: usize,
    limit: usize,
}

#[cfg(any(target_os = "macos", test))]
impl RenderFailures {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let rate = usize::try_from(sample_rate).unwrap_or(usize::MAX);
        Self {
            run: 0,
            cycles: 0,
            limit: (rate.saturating_mul(RENDER_FAILURE_TOLERANCE_MS) / 1000).max(1),
        }
    }

    pub(crate) fn failed(&mut self, frames: usize) -> bool {
        self.run = self.run.saturating_add(frames.max(1));
        self.cycles = self.cycles.saturating_add(1);
        self.cycles > 1 && self.run >= self.limit
    }

    pub(crate) const fn recovered(&mut self) {
        self.run = 0;
        self.cycles = 0;
    }
}

#[cfg(any(target_os = "macos", test))]
const MAX_CAPTURE_BACKLOG: usize = MAX_CALLBACK_FRAMES + 2 * crate::WORKER_BLOCK_SAMPLES;

#[cfg(any(target_os = "macos", test))]
pub(crate) fn shed_capture_backlog(
    ring: &mut Consumer<f32>,
    keep: usize,
    dropped: &AtomicU64,
) -> usize {
    let queued = ring.slots();
    if queued <= MAX_CAPTURE_BACKLOG {
        return 0;
    }
    let stale = queued - keep.min(queued);
    let Ok(chunk) = ring.read_chunk(stale) else {
        return 0;
    };
    chunk.commit_all();
    dropped.fetch_add(u64::try_from(stale).unwrap_or(u64::MAX), Ordering::Relaxed);
    stale
}

/// See docs/adr/0002-engine-diagnostics.md#output-underrun-counting.
#[derive(Debug)]
pub struct OutputFeed {
    ring: Consumer<f32>,
    underruns: Arc<AtomicU64>,
    primed: bool,
    counts_shortfalls: bool,
}

impl OutputFeed {
    /// Wraps the consumer end of the output ring and the underrun counter
    /// the control plane reads.
    #[must_use]
    pub const fn new(ring: Consumer<f32>, underruns: Arc<AtomicU64>) -> Self {
        Self {
            ring,
            underruns,
            primed: false,
            counts_shortfalls: false,
        }
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) const fn primed_ring(ring: Consumer<f32>, underruns: Arc<AtomicU64>) -> Self {
        Self {
            ring,
            underruns,
            primed: true,
            counts_shortfalls: true,
        }
    }

    /// See docs/adr/0002-engine-diagnostics.md#output-underrun-counting.
    pub fn render(&mut self, samples: &mut [f32], channels: usize) {
        let ring = &mut self.ring;
        let mut short = false;
        let delivered = fill_dual_mono(samples, channels, || {
            let sample = ring.pop().ok();
            short |= sample.is_none();
            sample
        });
        self.primed |= delivered;
        if self.primed && (!delivered || (short && self.counts_shortfalls)) {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    fn assert_samples(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "sample count");
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (actual - expected).abs() < f32::EPSILON,
                "sample {index}: {actual} != {expected}"
            );
        }
    }

    #[test]
    fn capture_size_is_four_bytes_per_frame_within_capacity() {
        assert_eq!(capture_byte_size(0, MAX_CALLBACK_FRAMES), Some(0));
        assert_eq!(capture_byte_size(256, MAX_CALLBACK_FRAMES), Some(1_024));
        assert_eq!(
            capture_byte_size(MAX_CALLBACK_FRAMES, MAX_CALLBACK_FRAMES),
            Some(16_384)
        );
    }

    #[test]
    fn capture_size_refuses_frames_beyond_the_buffer() {
        assert_eq!(
            capture_byte_size(MAX_CALLBACK_FRAMES + 1, MAX_CALLBACK_FRAMES),
            None
        );
        assert_eq!(capture_byte_size(1, 0), None);
    }

    #[test]
    fn render_geometry_uses_the_buffer_channel_count() {
        let geometry = render_geometry(2_048, 2, 256);
        assert_eq!(
            geometry,
            RenderGeometry {
                channels: 2,
                frames: 256
            }
        );
        assert_eq!(geometry.samples(), 512);
        assert_eq!(
            render_geometry(2_048, 1, 256),
            RenderGeometry {
                channels: 1,
                frames: 256
            }
        );
    }

    #[test]
    fn render_geometry_never_exceeds_the_buffer() {
        assert_eq!(render_geometry(800, 2, 256).frames, 100);
        assert_eq!(render_geometry(804, 2, 256).frames, 100);
        assert_eq!(render_geometry(0, 2, 256).frames, 0);
    }

    #[test]
    fn render_geometry_treats_zero_channels_as_mono() {
        let geometry = render_geometry(1_024, 0, 256);
        assert_eq!(geometry.channels, 1);
        assert_eq!(geometry.frames, 256);
    }

    #[test]
    fn dual_mono_fill_duplicates_each_sample_across_its_frame() {
        let mut samples = [9.0_f32; 6];
        let mut source = [0.1_f32, 0.2, 0.3].into_iter();
        assert!(fill_dual_mono(&mut samples, 2, || source.next()));
        assert_samples(&samples, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    }

    #[test]
    fn dual_mono_fill_renders_silence_for_frames_it_cannot_serve() {
        let mut samples = [9.0_f32; 6];
        let mut source = std::iter::once(0.5_f32);
        assert!(fill_dual_mono(&mut samples, 3, || source.next()));
        assert_samples(&samples, &[0.5, 0.5, 0.5, 0.0, 0.0, 0.0]);

        let mut silent = [9.0_f32; 4];
        assert!(!fill_dual_mono(&mut silent, 2, || None));
        assert_samples(&silent, &[0.0; 4]);
    }

    #[test]
    fn dual_mono_fill_treats_zero_channels_as_mono() {
        let mut samples = [9.0_f32; 2];
        let mut source = [0.25_f32, 0.75].into_iter();
        assert!(fill_dual_mono(&mut samples, 0, || source.next()));
        assert_samples(&samples, &[0.25, 0.75]);
    }

    #[test]
    fn capture_feed_queues_samples_and_counts_what_a_full_ring_drops() {
        let (producer, mut consumer) = RingBuffer::new(4);
        let overruns = Arc::new(AtomicU64::new(0));
        let mut feed = CaptureFeed::new(producer, Arc::clone(&overruns));
        feed.push(&[0.1, 0.2, 0.3]);
        assert_eq!(overruns.load(Ordering::Relaxed), 0);
        feed.push(&[0.4, 0.5, 0.6]);
        assert_eq!(overruns.load(Ordering::Relaxed), 2, "two samples overflow");
        let queued: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();
        assert_samples(&queued, &[0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn capture_feed_silences_non_finite_samples() {
        let (producer, mut consumer) = RingBuffer::new(8);
        let overruns = Arc::new(AtomicU64::new(0));
        let mut feed = CaptureFeed::new(producer, Arc::clone(&overruns));
        feed.push(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.25]);
        let queued: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();
        assert_samples(&queued, &[0.5, 0.0, 0.0, 0.0, -0.25]);
        assert_eq!(overruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_isolated_render_failure_is_tolerated() {
        let mut failures = RenderFailures::new(48_000);
        assert!(!failures.failed(512));
        failures.recovered();
        for _ in 0..1_000 {
            assert!(!failures.failed(4_096), "one failure between successes");
            failures.recovered();
        }
    }

    #[test]
    fn one_failed_cycle_longer_than_the_tolerance_is_tolerated() {
        let mut narrowband = RenderFailures::new(8_000);
        for _ in 0..1_000 {
            assert!(
                !narrowband.failed(MAX_CALLBACK_FRAMES),
                "512 ms in one cycle"
            );
            narrowband.recovered();
        }
        assert!(!narrowband.failed(1_024), "128 ms in one cycle");
        assert!(narrowband.failed(1_024), "the next cycle failed as well");
    }

    #[test]
    fn render_failures_that_persist_for_the_tolerance_are_a_fault() {
        let mut failures = RenderFailures::new(48_000);
        let verdicts: Vec<bool> = (0..10).map(|_| failures.failed(512)).collect();
        assert_eq!(verdicts.iter().filter(|fault| **fault).count(), 1);
        assert!(verdicts[9], "4800 frames are 100 ms at 48 kHz");

        let mut telephony = RenderFailures::new(16_000);
        assert!(!telephony.failed(1_599));
        assert!(telephony.failed(1));

        let mut empty = RenderFailures::new(48_000);
        assert!((0..4_799).all(|_| !empty.failed(0)));
        assert!(empty.failed(0), "an empty failed cycle still counts");
    }

    #[test]
    fn lost_capture_frames_are_counted_as_input_that_never_arrived() {
        let (producer, _consumer) = RingBuffer::new(8);
        let overruns = Arc::new(AtomicU64::new(0));
        let feed = CaptureFeed::new(producer, Arc::clone(&overruns));
        feed.lose(512);
        assert_eq!(overruns.load(Ordering::Relaxed), 512);
    }

    #[test]
    fn a_backlog_within_the_limit_is_left_alone() {
        let (mut producer, mut consumer) = RingBuffer::new(MAX_CAPTURE_BACKLOG * 2);
        let dropped = AtomicU64::new(0);
        for _ in 0..MAX_CAPTURE_BACKLOG {
            producer.push(0.5_f32).expect("ring has room");
        }
        assert_eq!(shed_capture_backlog(&mut consumer, 480, &dropped), 0);
        assert_eq!(consumer.slots(), MAX_CAPTURE_BACKLOG);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_backlog_past_the_limit_keeps_only_the_newest_samples() {
        let (mut producer, mut consumer) = RingBuffer::new(MAX_CAPTURE_BACKLOG * 2);
        let dropped = AtomicU64::new(0);
        let total = MAX_CAPTURE_BACKLOG + 1_000;
        for index in 0..total {
            let marker = if index < total - 480 { 0.0_f32 } else { 1.0 };
            producer.push(marker).expect("ring has room");
        }
        assert_eq!(
            shed_capture_backlog(&mut consumer, 480, &dropped),
            total - 480
        );
        assert_eq!(dropped.load(Ordering::Relaxed), (total - 480) as u64);
        let kept: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();
        assert_samples(&kept, &[1.0; 480]);
    }

    #[test]
    fn output_feed_counts_only_complete_starvation_after_priming() {
        let (mut producer, consumer) = RingBuffer::new(8);
        let underruns = Arc::new(AtomicU64::new(0));
        let mut feed = OutputFeed::new(consumer, Arc::clone(&underruns));
        let mut stereo = [9.0_f32; 4];

        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.0; 4]);
        assert_eq!(underruns.load(Ordering::Relaxed), 0);

        producer.push(0.5).expect("ring has room");
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.5, 0.5, 0.0, 0.0]);
        assert_eq!(underruns.load(Ordering::Relaxed), 0);

        feed.render(&mut stereo, 2);
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.0; 4]);
        assert_eq!(underruns.load(Ordering::Relaxed), 2);

        producer.push(0.25).expect("ring has room");
        producer.push(-0.25).expect("ring has room");
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.25, 0.25, -0.25, -0.25]);
        assert_eq!(underruns.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_feed_over_a_primed_ring_counts_every_shortfall() {
        let (mut producer, consumer) = RingBuffer::new(8);
        let underruns = Arc::new(AtomicU64::new(0));
        let mut feed = OutputFeed::primed_ring(consumer, Arc::clone(&underruns));
        let mut stereo = [9.0_f32; 4];

        producer.push(0.5).expect("ring has room");
        producer.push(0.5).expect("ring has room");
        feed.render(&mut stereo, 2);
        assert_eq!(underruns.load(Ordering::Relaxed), 0);

        producer.push(0.25).expect("ring has room");
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.25, 0.25, 0.0, 0.0]);
        assert_eq!(underruns.load(Ordering::Relaxed), 1);

        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.0; 4]);
        assert_eq!(underruns.load(Ordering::Relaxed), 2);
    }
}
