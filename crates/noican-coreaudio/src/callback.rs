//! Buffer geometry and ring hand-offs of the real-time callbacks.
//!
//! The arithmetic that sizes the preallocated capture landing buffer and
//! bounds what a render callback may write, and the two hand-offs every
//! callback performs: queueing captured samples for the inference worker
//! ([`CaptureFeed`]) and rendering the worker's output as dual mono
//! ([`OutputFeed`], [`fill_dual_mono`]). Pure and platform-independent so
//! the workspace tests cover them; the AUHAL callbacks in the `macos`
//! module only apply them.
//!
//! Real-time rules (docs/tech-research.md §9): a callback never
//! allocates, so the capture buffer is sized once at start from
//! [`MAX_CALLBACK_FRAMES`] — which is also set as
//! `kAudioUnitProperty_MaximumFramesPerSlice` on the unit — and a callback
//! asking for more frames than that is refused (fault flag, `paramErr`)
//! rather than served from a fresh allocation. The hand-offs only move
//! `f32` samples through preallocated lock-free rings and bump relaxed
//! atomic counters: ring overrun drops samples, underrun renders silence,
//! and neither ever blocks.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer};

/// Largest callback an AUHAL unit may deliver, in frames. Set as
/// `kAudioUnitProperty_MaximumFramesPerSlice` on every unit that
/// captures, and the length of each preallocated capture landing buffer.
pub const MAX_CALLBACK_FRAMES: usize = 4_096;

/// Byte size of a mono `f32` capture landing buffer holding `frames`,
/// or `None` when `frames` exceeds `capacity` (the buffer's length) —
/// the callback must then fault instead of rendering past the buffer.
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

/// Bounds a render request to the buffer AUHAL handed over.
///
/// `frame_count` frames of `number_channels` interleaved `f32` channels,
/// never more than `data_byte_size` bytes. A zero channel count is treated
/// as mono so a malformed buffer still yields silence rather than a
/// division by zero.
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

/// Writes one mono engine sample per frame into an interleaved render
/// buffer, duplicated into all of the frame's `channels` (dual mono).
///
/// This is the shape all three render callbacks (aggregate, split
/// output, preview monitor) produce. `next` supplies the samples, one
/// call per frame; a frame it cannot serve (`None`) renders silence. A
/// zero channel count is treated as mono, as in [`render_geometry`].
/// Returns whether any frame received a real sample.
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

/// The capture side's hand-off to the inference worker: pushes captured
/// samples into the capture ring, counting the ones a full ring drops.
///
/// A full ring means the worker (or the capture side) fell behind by more
/// than the whole ring, so those samples are dropped on the floor rather
/// than blocking the callback. A count that moves means real audio was
/// silently lost — audible as clipped input reaching the models — which
/// the frame heartbeat alone cannot show.
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

    /// Queues `samples` for the worker. Never blocks; each dropped sample
    /// is counted with a relaxed add (real-time safe).
    pub fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            if self.ring.push(sample).is_err() {
                self.overruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The output side of a render callback on either transport: pops the
/// worker's output ring into the device buffer as dual mono
/// ([`fill_dual_mono`]) and counts complete starvation as underrun.
///
/// A callback counts as an underrun only when it delivered no real sample
/// at all *after* the ring first produced audio. The ramp-up before that
/// is start-up latency, not underrun: the aggregate transport's ring
/// starts empty (the worker needs one full block of input before it
/// produces anything), so counting it would flag every model, including
/// ones that never miss the budget. Partial shortfalls are excluded on
/// purpose too: the worker writes 480-sample blocks into (typically)
/// smaller I/O periods, so a few benign partial zero-fills occur on every
/// model while the ring's phase cushion forms. A fully starved callback,
/// by contrast, means the worker fell a whole I/O period behind — audible
/// as dropouts in recordings from the virtual microphone.
#[derive(Debug)]
pub struct OutputFeed {
    ring: Consumer<f32>,
    underruns: Arc<AtomicU64>,
    /// Whether any sample has ever been popped. Callback-thread-only
    /// state, hence no atomic.
    primed: bool,
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
        }
    }

    /// Renders one callback into `samples` (interleaved, `channels` per
    /// frame); frames the ring cannot serve render silence. Never blocks;
    /// an underrun is counted with a relaxed add (real-time safe).
    pub fn render(&mut self, samples: &mut [f32], channels: usize) {
        let ring = &mut self.ring;
        if fill_dual_mono(samples, channels, || ring.pop().ok()) {
            self.primed = true;
        } else if self.primed {
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
        // 256 stereo frames: 256 * 2 * 4 bytes.
        let geometry = render_geometry(2_048, 2, 256);
        assert_eq!(
            geometry,
            RenderGeometry {
                channels: 2,
                frames: 256
            }
        );
        assert_eq!(geometry.samples(), 512);
        // Mono virtual output: same buffer holds twice the frames, but
        // the request caps it.
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
        // Buffer for 100 stereo frames, 256 requested: only 100 written.
        assert_eq!(render_geometry(800, 2, 256).frames, 100);
        // A trailing partial frame is dropped.
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
    fn output_feed_counts_only_complete_starvation_after_priming() {
        let (mut producer, consumer) = RingBuffer::new(8);
        let underruns = Arc::new(AtomicU64::new(0));
        let mut feed = OutputFeed::new(consumer, Arc::clone(&underruns));
        let mut stereo = [9.0_f32; 4];

        // Start-up ramp: the ring has never produced audio, so the
        // silence is latency, not underrun.
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.0; 4]);
        assert_eq!(underruns.load(Ordering::Relaxed), 0);

        // The first audio primes the feed; a partial shortfall is benign.
        producer.push(0.5).expect("ring has room");
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.5, 0.5, 0.0, 0.0]);
        assert_eq!(underruns.load(Ordering::Relaxed), 0);

        // After priming, every fully starved callback counts once.
        feed.render(&mut stereo, 2);
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.0; 4]);
        assert_eq!(underruns.load(Ordering::Relaxed), 2);

        // Real audio again: rendered as dual mono, nothing counted.
        producer.push(0.25).expect("ring has room");
        producer.push(-0.25).expect("ring has room");
        feed.render(&mut stereo, 2);
        assert_samples(&stereo, &[0.25, 0.25, -0.25, -0.25]);
        assert_eq!(underruns.load(Ordering::Relaxed), 2);
    }
}
