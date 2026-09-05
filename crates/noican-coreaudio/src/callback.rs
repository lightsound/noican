//! Buffer geometry of the real-time callbacks.
//!
//! The arithmetic that sizes the preallocated capture landing buffer and
//! bounds what a render callback may write. Pure and platform-independent
//! so the workspace tests cover it; the AUHAL callbacks in the `macos`
//! module only apply the results.
//!
//! Real-time rules (docs/tech-research.md §9): a callback never
//! allocates, so the capture buffer is sized once at start from
//! [`MAX_CALLBACK_FRAMES`] — which is also set as
//! `kAudioUnitProperty_MaximumFramesPerSlice` on the unit — and a callback
//! asking for more frames than that is refused (fault flag, `paramErr`)
//! rather than served from a fresh allocation.

use std::mem::size_of;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
