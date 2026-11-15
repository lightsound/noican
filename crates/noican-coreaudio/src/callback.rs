//! See docs/adr/0001-realtime-audio-rules.md.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer};

/// See docs/adr/0003-coreaudio-transport.md.
pub const MAX_CALLBACK_FRAMES: usize = 4_096;

/// See docs/adr/0003-coreaudio-transport.md.
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

/// See docs/adr/0003-coreaudio-transport.md.
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

/// See docs/adr/0004-aggregate-output-routing.md.
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

/// See docs/adr/0003-coreaudio-transport.md.
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

    /// See docs/adr/0003-coreaudio-transport.md.
    pub fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            if self.ring.push(sample).is_err() {
                self.overruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// See docs/adr/0002-engine-diagnostics.md.
#[derive(Debug)]
pub struct OutputFeed {
    ring: Consumer<f32>,
    underruns: Arc<AtomicU64>,
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

    /// See docs/adr/0002-engine-diagnostics.md.
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
}
