//! Preview (self-monitor) policy and worker-side machinery.
//!
//! Platform-independent by design so its invariants are unit-tested on
//! every CI target: the [`MonitorTee`] (the inference worker's half of the
//! monitor, including the [`HowlGuard`] feedback killswitch) and
//! [`classify_monitor_target`] (the safety decision about which output
//! devices may receive the preview). The macOS-only counterpart — the
//! monitor AUHAL lifecycle — lives in the `macos` transport module and
//! consumes both.
//!
//! Real-time rules (docs/tech-research.md §9): [`MonitorTee::feed`] runs
//! on the inference worker between device callbacks — no allocation, no
//! locks; the gate flags are plain atomics and samples go into a
//! preallocated lock-free ring.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::Producer;

use crate::CoreAudioError;

/// Monitor ring capacity: 100 ms at 48 kHz.
///
/// Deliberately small — the monitor AUHAL runs on the playback device's
/// own clock and drift is not corrected, so a slow drain pins the ring at
/// its capacity. A small ring caps the preview latency at ~100 ms and
/// turns the drift into an occasional discarded block instead (accepted
/// preview artifact).
pub const MONITOR_RING_CAPACITY: usize = 4_800;

/// Samples the monitor ring must hold before playback (re)starts after an
/// underrun: 40 ms at 48 kHz. Priming turns scattered single-sample
/// underruns into one bounded silence gap.
pub const MONITOR_PRIME_SAMPLES: usize = 1_920;

/// Peak level (linear) a teed block must reach to count toward a trip.
///
/// Acoustic feedback has loop gain above one, so it grows until it
/// saturates near full scale; speech peaks touch this level but do not
/// hold it continuously.
pub const HOWL_PEAK_THRESHOLD: f32 = 0.98;

/// Consecutive near-clipping blocks (10 ms each) before a trip: 500 ms.
///
/// Long enough that shouting into the microphone does not trip it
/// (speech dips between syllables), short enough to cut a howl before it
/// is painful.
pub const HOWL_TRIP_BLOCKS: usize = 50;

/// Four-character Core Audio code as a big-endian `u32`.
#[must_use]
pub const fn fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}

/// `kAudioDeviceTransportTypeVirtual`: loopback drivers (`BlackHole`,
/// JoyCast, ...).
pub const TRANSPORT_TYPE_VIRTUAL: u32 = fourcc(*b"virt");
/// `kAudioDeviceTransportTypeAggregate`: aggregate and Multi-Output
/// devices composed in Audio MIDI Setup.
pub const TRANSPORT_TYPE_AGGREGATE: u32 = fourcc(*b"grup");
/// `kAudioDeviceTransportTypeBuiltIn`: the Mac's own output.
pub const TRANSPORT_TYPE_BUILT_IN: u32 = fourcc(*b"bltn");
/// Built-in output data source for the internal speakers
/// (`kAudioDevicePropertyDataSource` value `'ispk'`; the headphone jack
/// reports `'hdpn'`).
pub const DATA_SOURCE_INTERNAL_SPEAKER: u32 = fourcc(*b"ispk");

/// Decides whether an output device may receive the preview.
///
/// Refused targets:
/// - **Virtual loopbacks** (`virt` transport or a `BlackHole`/Noican UID)
///   — the processed voice would reach the meeting a second time.
/// - **Aggregate / Multi-Output devices** (`grup` transport) — they can
///   contain the meeting loopback as a subdevice, which this check cannot
///   cheaply inspect, and the feedback guard cannot catch that route
///   (it is not an acoustic loop).
/// - **The built-in internal speakers** (`bltn` transport with the
///   `ispk` data source) — the voice would feed straight back into the
///   microphone (Phase 0/1 has no echo cancellation).
///
/// Everything else — Bluetooth, USB, HDMI, the built-in headphone jack,
/// or unreadable properties (`transport == 0`, `data_source == None`) —
/// fails open: those cannot be classified reliably, and the worker-side
/// [`HowlGuard`] is the safety net for acoustic feedback through them.
///
/// The UID predicate must stay aligned with the Swift picker's
/// `AudioDeviceCatalog.isNoicanVirtualDevice`
/// (macos/Sources/NoicanMenuBar/CoreAudioDevices.swift); the Rust side is
/// deliberately broader (any virtual/aggregate transport).
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

/// Last-resort feedback killswitch for the preview monitor.
///
/// Trips when the teed output holds near clipping for
/// [`HOWL_TRIP_BLOCKS`] consecutive blocks — the signature of the preview
/// playing through speakers back into the microphone. Complements
/// [`classify_monitor_target`], which cannot classify every output.
///
/// Real-time safe: one fold over the observed block, no allocation, no
/// locks. Owned by [`MonitorTee`] on the inference worker.
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

    /// Observes one teed block. Returns `true` when the guard trips; the
    /// caller must then stop feeding the monitor. The run resets on any
    /// block below the threshold and after a trip.
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
/// Owns the ring producer, the lock-free gate flags shared with the
/// control plane, and the feedback guard together, so the worker calls
/// one method per block.
#[derive(Debug)]
pub struct MonitorTee {
    producer: Producer<f32>,
    /// Armed/disarmed by the control plane; cleared here on a trip.
    enabled: Arc<AtomicBool>,
    /// Raised here on a trip; cleared by the control plane on the next
    /// monitor toggle in either direction.
    tripped: Arc<AtomicBool>,
    howl: HowlGuard,
}

impl MonitorTee {
    /// Bundles the worker half around a preallocated ring producer and
    /// the flags shared with the control plane.
    #[must_use]
    pub const fn new(
        producer: Producer<f32>,
        enabled: Arc<AtomicBool>,
        tripped: Arc<AtomicBool>,
    ) -> Self {
        Self {
            producer,
            enabled,
            tripped,
            howl: HowlGuard::new(),
        }
    }

    /// Feeds one processed block into the monitor ring when armed,
    /// without blocking.
    ///
    /// Ring overrun (a monitor device draining slower than the engine
    /// clock) silently discards the overflowing samples: preview
    /// tolerates minor artifacts and must never push back on the
    /// meeting-facing path. When the fed audio holds near clipping long
    /// enough for the [`HowlGuard`] to trip, the tee disarms itself
    /// immediately and raises the tripped flag for the control plane.
    /// While disarmed the ring is not touched at all. Returns whether the
    /// block was teed, so both branches are observable in tests.
    pub fn feed(&mut self, block: &[f32]) -> bool {
        if !self.enabled.load(Ordering::Acquire) {
            self.howl.reset();
            return false;
        }
        for &sample in block {
            let _overrun_discards = self.producer.push(sample);
        }
        if self.howl.observe(block) {
            self.enabled.store(false, Ordering::Release);
            self.tripped.store(true, Ordering::Release);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    fn tee(
        capacity: usize,
        enabled: bool,
    ) -> (
        MonitorTee,
        rtrb::Consumer<f32>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
    ) {
        let (producer, consumer) = RingBuffer::new(capacity);
        let enabled = Arc::new(AtomicBool::new(enabled));
        let tripped = Arc::new(AtomicBool::new(false));
        (
            MonitorTee::new(producer, Arc::clone(&enabled), Arc::clone(&tripped)),
            consumer,
            enabled,
            tripped,
        )
    }

    #[test]
    fn tee_delivers_identical_samples_when_enabled() {
        let (mut tee, mut consumer, _enabled, _tripped) = tee(16, true);
        let block = [0.1_f32, -0.5, 0.25, 1.0];
        assert!(tee.feed(&block));
        for &expected in &block {
            let delivered = consumer.pop().expect("teed sample is present");
            assert!((delivered - expected).abs() < f32::EPSILON);
        }
        assert!(consumer.pop().is_err(), "no extra samples were teed");
    }

    #[test]
    fn tee_is_skipped_when_disabled() {
        let (mut tee, mut consumer, _enabled, _tripped) = tee(16, false);
        assert!(!tee.feed(&[0.5, -0.5]));
        assert!(consumer.pop().is_err(), "disarmed tee must not write");
    }

    #[test]
    fn tee_overrun_discards_instead_of_blocking() {
        let (mut tee, mut consumer, _enabled, _tripped) = tee(2, true);
        let block = [1.0_f32, 2.0, 3.0, 4.0];
        assert!(tee.feed(&block));
        // The first two samples fit; the overflow is dropped, not queued.
        assert!((consumer.pop().expect("first sample") - 1.0).abs() < f32::EPSILON);
        assert!((consumer.pop().expect("second sample") - 2.0).abs() < f32::EPSILON);
        assert!(consumer.pop().is_err());
    }

    #[test]
    fn sustained_clipping_disarms_the_tee_and_raises_the_trip_flag() {
        let (mut tee, _consumer, enabled, tripped) = tee(4, true);
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(tee.feed(&loud));
            assert!(!tripped.load(Ordering::Acquire), "below the trip length");
        }
        assert!(tee.feed(&loud), "the tripping block itself is still teed");
        assert!(tripped.load(Ordering::Acquire), "trip flag raised");
        assert!(!enabled.load(Ordering::Acquire), "tee disarmed itself");
        assert!(!tee.feed(&loud), "no further blocks are teed");
    }

    #[test]
    fn rearming_after_a_trip_requires_a_fresh_sustained_run() {
        let (mut tee, _consumer, enabled, tripped) = tee(4, true);
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS {
            let _teed = tee.feed(&loud);
        }
        assert!(tripped.load(Ordering::Acquire));
        // Control plane re-arms (as Runtime::set_monitor(true) does).
        tripped.store(false, Ordering::Release);
        enabled.store(true, Ordering::Release);
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            assert!(tee.feed(&loud));
            assert!(!tripped.load(Ordering::Acquire), "run restarted from zero");
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
        // The run resets after a trip: a fresh sustained run is required.
        assert!(!guard.observe(&loud));
    }

    #[test]
    fn howl_guard_resets_on_quiet_blocks_and_explicit_reset() {
        let mut guard = HowlGuard::new();
        let loud = [1.0_f32; 4];
        for _ in 0..HOWL_TRIP_BLOCKS - 1 {
            let _tripped = guard.observe(&loud);
        }
        // One block below the threshold clears the run.
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
        // Peaks touch full scale but dip every few blocks, like speech.
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
        // BlackHole UIDs are refused on any transport (belt and braces).
        assert!(matches!(
            classify_monitor_target(0, "BlackHole2ch_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        assert!(matches!(
            classify_monitor_target(0, "BlackHole16ch_UID", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
        // The Noican fork prefix matches case-insensitively.
        assert!(matches!(
            classify_monitor_target(0, "COM.LIGHTSOUND.NOICAN.2ch", None),
            Err(CoreAudioError::MonitorLoopbackOutput { .. })
        ));
    }

    #[test]
    fn classify_refuses_aggregate_and_multi_output_devices() {
        // Multi-Output devices report the aggregate transport and an
        // AMS-generated UID; they can hide the meeting loopback inside.
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
        // Bluetooth/USB/unknown transports and unreadable properties are
        // allowed: the HowlGuard is the documented safety net there.
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
