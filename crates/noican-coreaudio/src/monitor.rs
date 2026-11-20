//! See docs/adr/0005-preview-monitor.md.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use rtrb::Producer;

use crate::CoreAudioError;

/// See docs/adr/0005-preview-monitor.md.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorState {
    /// See docs/adr/0005-preview-monitor.md.
    #[default]
    Off = 0,
    /// The monitor AUHAL is up and the worker tees processed audio into
    /// its ring.
    Playing = 1,
    /// See docs/adr/0005-preview-monitor.md.
    Tripped = 2,
}

impl MonitorState {
    /// See docs/adr/0005-preview-monitor.md.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Playing,
            2 => Self::Tripped,
            _ => Self::Off,
        }
    }

    /// The C-ABI value of this state.
    #[must_use]
    pub const fn as_raw(self) -> i32 {
        self as i32
    }
}

/// See docs/adr/0005-preview-monitor.md.
pub const MONITOR_RING_CAPACITY: usize = 4_800;

/// See docs/adr/0005-preview-monitor.md.
pub const MONITOR_PRIME_SAMPLES: usize = 1_920;

/// See docs/adr/0005-preview-monitor.md.
pub const HOWL_PEAK_THRESHOLD: f32 = 0.98;

/// See docs/adr/0005-preview-monitor.md.
pub const HOWL_TRIP_BLOCKS: usize = 50;

/// Four-character Core Audio code as a big-endian `u32`.
#[must_use]
pub const fn fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}

/// See docs/adr/0003-coreaudio-transport.md.
pub const TRANSPORT_TYPE_VIRTUAL: u32 = fourcc(*b"virt");
/// See docs/adr/0003-coreaudio-transport.md.
pub const TRANSPORT_TYPE_AGGREGATE: u32 = fourcc(*b"grup");
/// See docs/adr/0003-coreaudio-transport.md.
pub const TRANSPORT_TYPE_BUILT_IN: u32 = fourcc(*b"bltn");
/// See docs/adr/0003-coreaudio-transport.md.
pub const DATA_SOURCE_INTERNAL_SPEAKER: u32 = fourcc(*b"ispk");

/// See docs/adr/0005-preview-monitor.md.
///
/// # Errors
///
/// Returns the matching [`CoreAudioError`] refusal for the cases above.
pub fn classify_monitor_target(
    transport: u32,
    uid: &str,
    data_source: Option<u32>,
) -> Result<(), CoreAudioError> {
    if transport == TRANSPORT_TYPE_VIRTUAL || is_noican_loopback_uid(uid) {
        return Err(CoreAudioError::MonitorLoopbackOutput {
            uid: uid.to_owned(),
        });
    }
    if transport == TRANSPORT_TYPE_AGGREGATE {
        return Err(CoreAudioError::MonitorAggregateOutput {
            uid: uid.to_owned(),
        });
    }
    if transport == TRANSPORT_TYPE_BUILT_IN && data_source == Some(DATA_SOURCE_INTERNAL_SPEAKER) {
        return Err(CoreAudioError::MonitorSpeakerOutput);
    }
    Ok(())
}

fn is_noican_loopback_uid(uid: &str) -> bool {
    uid.contains("BlackHole") || uid.to_lowercase().starts_with("com.lightsound.noican.")
}

/// See docs/adr/0005-preview-monitor.md.
#[derive(Debug, Default)]
pub struct HowlGuard {
    consecutive: usize,
}

impl HowlGuard {
    /// Creates a guard with no accumulated run.
    #[must_use]
    pub const fn new() -> Self {
        Self { consecutive: 0 }
    }

    /// See docs/adr/0005-preview-monitor.md.
    pub fn observe(&mut self, block: &[f32]) -> bool {
        let peak = block.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        if peak >= HOWL_PEAK_THRESHOLD {
            self.consecutive += 1;
            if self.consecutive >= HOWL_TRIP_BLOCKS {
                self.consecutive = 0;
                return true;
            }
        } else {
            self.consecutive = 0;
        }
        false
    }

    /// Clears the accumulated run (monitoring is disabled).
    pub const fn reset(&mut self) {
        self.consecutive = 0;
    }
}

/// The inference worker's half of the preview monitor.
///
/// Owns the ring producer, the shared lock-free [`MonitorState`] cell,
/// and the feedback guard together, so the worker calls one method per
/// block.
#[derive(Debug)]
pub struct MonitorTee {
    producer: Producer<f32>,
    state: Arc<AtomicI32>,
    howl: HowlGuard,
}

impl MonitorTee {
    /// Bundles the worker half around a preallocated ring producer and
    /// the [`MonitorState`] cell shared with the control plane.
    #[must_use]
    pub const fn new(producer: Producer<f32>, state: Arc<AtomicI32>) -> Self {
        Self {
            producer,
            state,
            howl: HowlGuard::new(),
        }
    }

    /// See docs/adr/0005-preview-monitor.md.
    pub fn feed(&mut self, block: &[f32]) -> bool {
        if MonitorState::from_raw(self.state.load(Ordering::Acquire)) != MonitorState::Playing {
            self.howl.reset();
            return false;
        }
        for &sample in block {
            let _overrun_discards = self.producer.push(sample);
        }
        if self.howl.observe(block) {
            self.state
                .store(MonitorState::Tripped.as_raw(), Ordering::Release);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    fn read_state(state: &Arc<AtomicI32>) -> MonitorState {
        MonitorState::from_raw(state.load(Ordering::Acquire))
    }

    fn tee(
        capacity: usize,
        initial: MonitorState,
    ) -> (MonitorTee, rtrb::Consumer<f32>, Arc<AtomicI32>) {
        let (producer, consumer) = RingBuffer::new(capacity);
        let state = Arc::new(AtomicI32::new(initial.as_raw()));
        (
            MonitorTee::new(producer, Arc::clone(&state)),
            consumer,
            state,
        )
    }

    #[test]
    fn monitor_state_round_trips_and_defaults_to_off() {
        for state in [
            MonitorState::Off,
            MonitorState::Playing,
            MonitorState::Tripped,
        ] {
            assert_eq!(MonitorState::from_raw(state.as_raw()), state);
        }
        assert_eq!(MonitorState::from_raw(-1), MonitorState::Off);
        assert_eq!(MonitorState::from_raw(3), MonitorState::Off);
        assert_eq!(MonitorState::Off.as_raw(), 0);
        assert_eq!(MonitorState::Playing.as_raw(), 1);
        assert_eq!(MonitorState::Tripped.as_raw(), 2);
    }

    #[test]
    fn tee_delivers_identical_samples_when_playing() {
        let (mut tee, mut consumer, _state) = tee(16, MonitorState::Playing);
        let block = [0.1_f32, -0.5, 0.25, 1.0];
        assert!(tee.feed(&block));
        for &expected in &block {
            let delivered = consumer.pop().expect("teed sample is present");
            assert!((delivered - expected).abs() < f32::EPSILON);
        }
        assert!(consumer.pop().is_err(), "no extra samples were teed");
    }

    #[test]
    fn tee_is_skipped_unless_playing() {
        for gate in [MonitorState::Off, MonitorState::Tripped] {
            let (mut idle_tee, mut consumer, _state) = tee(16, gate);
            assert!(!idle_tee.feed(&[0.5, -0.5]));
            assert!(
                consumer.pop().is_err(),
                "a tee gated by {gate:?} must not write"
            );
        }
    }

    #[test]
    fn tee_overrun_discards_instead_of_blocking() {
        let (mut tee, mut consumer, _state) = tee(2, MonitorState::Playing);
        let block = [1.0_f32, 2.0, 3.0, 4.0];
        assert!(tee.feed(&block));
        assert!((consumer.pop().expect("first sample") - 1.0).abs() < f32::EPSILON);
        assert!((consumer.pop().expect("second sample") - 2.0).abs() < f32::EPSILON);
        assert!(consumer.pop().is_err());
    }

    #[test]
    fn sustained_clipping_moves_the_state_to_tripped() {
        let (mut tee, _consumer, state) = tee(4, MonitorState::Playing);
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(tee.feed(&loud));
            assert_eq!(
                read_state(&state),
                MonitorState::Playing,
                "below the trip length"
            );
        }
        assert!(tee.feed(&loud), "the tripping block itself is still teed");
        assert_eq!(
            read_state(&state),
            MonitorState::Tripped,
            "the single store both disarms and reports"
        );
        assert!(!tee.feed(&loud), "no further blocks are teed");
    }

    #[test]
    fn rearming_after_a_trip_requires_a_fresh_sustained_run() {
        let (mut tee, _consumer, state) = tee(4, MonitorState::Playing);
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS {
            let _teed = tee.feed(&loud);
        }
        assert_eq!(read_state(&state), MonitorState::Tripped);
        state.store(MonitorState::Playing.as_raw(), Ordering::Release);
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(tee.feed(&loud));
            assert_eq!(
                read_state(&state),
                MonitorState::Playing,
                "run restarted from zero"
            );
        }
    }

    #[test]
    fn howl_guard_trips_only_after_a_sustained_run() {
        let mut guard = HowlGuard::new();
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(!guard.observe(&loud), "below the trip length");
        }
        assert!(guard.observe(&loud), "trips exactly at the trip length");
        assert!(!guard.observe(&loud));
    }

    #[test]
    fn howl_guard_resets_on_quiet_blocks_and_explicit_reset() {
        let mut guard = HowlGuard::new();
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            let _tripped = guard.observe(&loud);
        }
        assert!(!guard.observe(&[0.5_f32; 4]));
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(!guard.observe(&loud), "run restarted from zero");
        }
        guard.reset();
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(!guard.observe(&loud), "reset cleared the run");
        }
    }

    #[test]
    fn howl_guard_ignores_loud_speech_with_dips() {
        let mut guard = HowlGuard::new();
        for _ in 0..20 {
            for _ in 0..HOWL_TRIP_BLOCKS / 2 {
                assert!(!guard.observe(&[1.0_f32; 4]));
            }
            assert!(!guard.observe(&[0.2_f32; 4]));
        }
    }

    #[test]
    fn classify_refuses_virtual_and_loopback_uids() {
        assert!(matches!(
            classify_monitor_target(TRANSPORT_TYPE_VIRTUAL, "JoyCastDevice_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "BlackHole2ch_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "BlackHole16ch_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "COM.LIGHTSOUND.NOICAN.mic", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "com.lightsound.noican.mic_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "com.lightsound.noican.mic_2_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "com.lightsound.noican.2ch_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "com.lightsound.noican.2ch_2_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(classify_monitor_target(0, "com.lightsound.noican_UID", None).is_ok());
    }

    #[test]
    fn classify_refuses_aggregate_and_multi_output_devices() {
        assert!(matches!(
            classify_monitor_target(TRANSPORT_TYPE_AGGREGATE, "~:AMS2_StackedOutput:0", None),
            Err(CoreAudioError::MonitorAggregateOutput { .. })
        ));
    }

    #[test]
    fn classify_refuses_internal_speakers_but_allows_the_headphone_jack() {
        assert!(matches!(
            classify_monitor_target(
                TRANSPORT_TYPE_BUILT_IN,
                "BuiltInSpeakerDevice",
                Some(DATA_SOURCE_INTERNAL_SPEAKER)
            ),
            Err(CoreAudioError::MonitorSpeakerOutput)
        ));
        let headphone_jack = fourcc(*b"hdpn");
        assert!(
            classify_monitor_target(
                TRANSPORT_TYPE_BUILT_IN,
                "BuiltInHeadphoneOutputDevice",
                Some(headphone_jack)
            )
            .is_ok()
        );
    }

    #[test]
    fn classify_fails_open_where_it_cannot_classify() {
        let bluetooth = fourcc(*b"blue");
        let usb = fourcc(*b"usb ");
        assert!(classify_monitor_target(bluetooth, "AB-CD-EF-01-02-03:output", None).is_ok());
        assert!(classify_monitor_target(usb, "SomeUSBDAC_UID", Some(0)).is_ok());
        assert!(classify_monitor_target(0, "", None).is_ok());
        assert!(
            classify_monitor_target(TRANSPORT_TYPE_BUILT_IN, "BuiltInSpeakerDevice", None).is_ok(),
            "unreadable data source fails open on the built-in device"
        );
    }
}
