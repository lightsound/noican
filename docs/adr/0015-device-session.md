# 0015. Devices, sample rates and session handling

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the source
location of each comment at the time of the move (commit 397e3df)
is noted above its text.

## capture-rate-acceptance

Source: `macos/NoicanState/Tests/NoicanStateTests/AppReducerTests.swift` lines 65–66.

Incapable microphone (a rate outside the capture resampler's
range — telephony-rate and 44.1 kHz microphones are accepted).

## capture-rate-device-classes

Source: `macos/NoicanState/Tests/NoicanStateTests/ReducerTestSupport.swift` lines 7–8.

A telephony-profile Bluetooth headset microphone: selectable since
issue #7 (captured natively at 16 kHz and resampled in the transport).

Source: `macos/NoicanState/Tests/NoicanStateTests/ReducerTestSupport.swift` lines 10–11.

A 44.1 kHz-only device (the owner's Bluetooth headset): selectable
since the capture resampler went rational — full-band, split path.

Source: `macos/NoicanState/Tests/NoicanStateTests/ReducerTestSupport.swift` lines 15–16.

A device whose rate lies outside the resampler's 8–192 kHz range —
the remaining refusal case (an unreadable rate reads as 0 Hz).

## capture-rate-validation

Source: `crates/noican-ffi/src/lib.rs` lines 346–355.

Validates a native capture rate: finite, whole hertz (Core Audio
nominal rates are exact integers in f64 — 44100.0, never 44100.3 —
so anything fractional is a bogus reading, not a rate to round), and
within the range the capture resampler converts
([`noican_core::capture::MIN_NATIVE_RATE`] to
[`noican_core::capture::MAX_NATIVE_RATE`]). Rejecting here — before
any weight download or audio object — keeps the failure instant and
its message precise. The reduced ratio's size is bounded by the
resampler itself ([`noican_core::capture::MAX_RATIO_TERM`]) when the
transport constructs it, still before any audio object.

Source: `crates/noican-ffi/src/lib.rs` lines 1420–1423.

Valid rates — a telephony profile and the 44.1 kHz family —
pass validation; on portable builds the transport itself then
refuses (and macOS refuses the nonexistent device 0), but
never with the rate message.

## capture-test-oracle

Source: `crates/noican-core/src/capture.rs` lines 265–270.

Deterministic speech-like test signal: a gliding fundamental with
harmonics and a syllabic amplitude envelope — broadband and
aperiodic enough for an unambiguous cross-correlation peak,
band-limited like telephony speech (content stays below ~1.3 kHz,
under every telephony profile's Nyquist). Defined in continuous
time, so two rates sample the same underlying waveform.

Source: `crates/noican-core/src/capture.rs` lines 291–293.

Lag (in samples) of the peak normalized cross-correlation of `b`
against `a` — `b` is `a` delayed by the returned lag — searched
over `0..=max_lag`, with the peak's correlation value.

## core-audio-listener-observation

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 4–12.

Core Audio property listeners the menu-bar app keeps in lockstep with
engine state: the preview monitor's per-device data-source watch (a
headphone-jack flip fires no device-list or default-output
notification) and the running microphone's nominal sample-rate watch
(Bluetooth headsets renegotiate A2DP ↔ HFP underneath a split
transport that fixed its rate at start). Both live here, off the main
`AppState.swift`, so that file stays under the lint length cap; the
shared state they touch is `internal` for the same reason (see
`allDevices`).

## core-audio-string-property-interop

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift` lines 231–233.

A properly typed, explicitly allocated landing slot: passing
`&value` of an `Optional<CFString>` local forms a raw pointer to a
non-trivial Swift-managed value, which the compiler warns about.

## device-catalog-scope

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 410–411.

The transport-loss check runs against every device with
input channels, not just the selectable ones.

## device-hot-plug-handling

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 420–422.

Follows device hot-plug: refreshes the picker automatically (the
reducer stops the engine when the microphone in use or the virtual
output disappears).

## device-hotplug-auto-follow

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 353–354.

Device hot-plug is followed automatically (AppState registers a
Core Audio device-list listener), so no manual refresh is needed.

## device-list-selectability-filter

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift` lines 71–75.

The device list changed (hot-plug) or was first read. `inputs` is
the selectable-microphone snapshot; `allInputUIDs` additionally
contains every device with input channels (the transport-loss
check must not depend on the selectability filter); the virtual
output presence is a start pre-flight.

## device-rate-label

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 417–419.

Non-48 kHz devices carry their native rate so the
conversion (and, for telephony profiles, the
narrow-band trade-off) is visible before selecting.

## driver-uid-matching

Source: `crates/noican-coreaudio/src/monitor.rs` lines 163–168.

The prefix (trailing dot included) covers every Noican driver UID:
the driver build (external/noican-driver/scripts/build-driver.sh) derives
the shipped device UID "com.lightsound.noican.mic_UID" (driver 0.2.0,
1 channel) from it, as it did the 0.1.0 driver's
"com.lightsound.noican.2ch_UID"; both stay
recognized while the two are swapped under one app build.

Source: `crates/noican-coreaudio/src/monitor.rs` lines 449–450.

The exact UID the Noican driver ships (its build-driver.sh
derives it from kDriver_Name="com.lightsound.noican.mic").

Source: `crates/noican-coreaudio/src/monitor.rs` lines 455–455.

The driver's hidden mirror device shares the prefix.

Source: `crates/noican-coreaudio/src/monitor.rs` lines 460–462.

The 0.1.0 (2-channel) driver's UIDs stay recognized: old and new
drivers are swapped under one app build during the transition
(docs/driver.md, "History").

Source: `crates/noican-coreaudio/src/monitor.rs` lines 471–473.

A base without the trailing dot would not be a Noican UID to
this matcher — the shape docs/driver.md warns the driver build
against.

## input-device-snapshot

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 61–61.

Core Audio device UID (stable across reconnects).

## input-rate-listener-lifecycle

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 101–111.

Keeps the nominal-rate listener in lockstep with the transport:
registered on the session's microphone when a native-capture
start settles (`activeCaptureRate` is the split path's marker)
and kept through busy monitor/model transitions — the transport
stays up during those, so keying on `liveSession` would drop the
listener for the length of every Preview toggle. Removed the
moment the transport is gone. The 48 kHz aggregate path never
registers one, so its behavior is unchanged. Bluetooth headsets
renegotiate profiles (A2DP ↔ HFP), and the split transport
captures at the rate fixed at start time, so a live change must
rebuild the transport.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 119–121.

The running microphone changed underneath us (a start
attempt switched mics while a listener survived from the
previous transport): rebind before registering.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 136–137.

The 1 Hz stall poll still catches a rate
renegotiation; log why the notification path is dead.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 142–147.

The effective rate settles at capture start (Bluetooth
renegotiates when the HFP link comes up), which can race
the start effect's read: compare once at attach time so a
renegotiation that landed during the start is caught now
rather than waiting for a notification that may never
fire again.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 154–156.

Drops the input-rate listener, if any. Removal of a device that
disappeared in the meantime fails harmlessly, so the status is
ignored.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift` lines 171–177.

Re-reads the running microphone's nominal rate and rebuilds the
transport (via the reducer) when it no longer matches the rate
the transport was started with. Comparing against the recorded
baseline filters spurious notifications; the busy machine
serializes overlapping rebuilds (a flip landing *during* a busy
transition is dropped here and caught by the attach-time check
or the 1 Hz health poll after the transition settles).

## input-rate-observation

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 93–101.

Watches the running session's microphone for nominal-rate changes
while the native-capture path is active: Bluetooth headsets
renegotiate between A2DP and HFP profiles, and the split
transport captures at the rate fixed at start time, so a change
means the transport must be rebuilt. Registered when a
native-capture start settles; always removed when the session
ends (`syncInputRateObservation`).
Internal for `ListenerObservation.swift`, which owns its
registration lifecycle.

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 488–490.

Belt and braces for the nominal-rate listener: a profile flip
landing during a busy transition is dropped by the listener's
busy guard, so the poll re-checks once per second.

## input-sample-rate-renegotiation

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift` lines 81–86.

The running microphone renegotiated its nominal sample rate
after the transport was built around the old one (Bluetooth
headsets flip between A2DP and HFP profiles). The shell only
dispatches this after confirming the rate actually differs from
the rate the transport captures at, so the reducer's job is to
rebuild the transport into the current mode.

## listener-registration-failure-logging

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 429–430.

A failed registration would leave topology changes silently
unnoticed — log the status instead of discarding it.

## live-channel-count-read

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift` lines 258–263.

Live output channel count of a device, or nil when its stream
configuration is unreadable. Read at aggregate-composition time
rather than taken from the device snapshot: the count can depend
on the sample rate (ADAT/S-MUX interfaces carry 8 channels at
48 kHz, 4 at 96 kHz, 2 at 192 kHz), and the snapshot predates the
switch to 48 kHz that composition performs first.

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift` lines 274–276.

Number of output streams a device exposes, or nil when unreadable.
An aggregate carries one stream per subdevice output; logged at
start so the AUHAL-reported channel count can be held against it.

## microphone-list-always-visible

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 323–327.

All selectable inputs as an always-visible, checkmarked list
(Control Center style) instead of a popup: newly connected devices
appear immediately (the device list follows hot-plug), and the
microphone can be switched while running — the engine rebuilds its
transport around the new device with the same model and mode.

## microphone-rate-support

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 696–703.

Why `device` cannot serve as the engine's microphone, or nil when
it can. Decided from the snapshot taken at device-refresh time —
no Core Audio call, so it is safe before any transition.
Non-48 kHz devices (Bluetooth headset microphones, 44.1 kHz-family
devices) are *not* refused: the transport captures them natively
and resamples by the exact ratio (issue #7). Only rates outside
the resampler's 8–192 kHz range — or an unreadable rate — remain
unusable.

## microphone-refusal-acknowledgement

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 215–218.

Re-clicking the already-selected microphone acknowledges a
shown refusal: the message describes a *rejected other*
device, so confirming the working one clears it (the
engine and the selection are untouched).

## native-capture-rate-baseline

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 86–91.

Rate the running transport captures at when the native-capture
(split) path is active, or nil on the 48 kHz aggregate path.
Recorded by the start effect and used as the baseline the
nominal-rate listener compares against.
Internal for `ListenerObservation.swift` (the nominal-rate
listener compares against it).

## native-rate-bounds

Source: `crates/noican-core/src/capture.rs` lines 45–47.

Lowest native capture rate the split transport accepts, in Hz: the
narrowest telephony profile (Bluetooth HFP narrow-band). Nothing
below it exists as a microphone format.

Source: `crates/noican-core/src/capture.rs` lines 50–56.

Highest native capture rate the split transport accepts, in Hz.

The resampler handles any ratio; this bounds its filter memory and
the transport's per-pass work (a 192 kHz device delivers four native
samples per engine sample). Devices above 48 kHz normally also
advertise 48 kHz and take the aggregate path; the bound is for the
odd interface that does not.

Source: `crates/noican-core/src/capture.rs` lines 59–68.

Largest numerator or denominator of the reduced native/engine ratio
the split transport accepts.

The polyphase bank grows with the reduced ratio (`up` phases × ~40
taps × `max(1, down / up)`): the real rate families stay small —
640/147 for 11.025 kHz is the largest, 1/4 for 192 kHz the deepest —
but a nominal rate coprime with 48 000 (e.g. 16 001 Hz) would reduce
to 48 000/16 001 and design a ~1.9 M-tap, 10 MB bank synchronously on
the start path. Such rates do not occur on real hardware, so they
are refused with a precise message instead of bounded by memory.

Source: `crates/noican-core/src/capture.rs` lines 84–94.

Creates a converter from `native_rate` Hz to the engine rate,
preallocating for input chunks of up to `max_input_len` native
samples per [`InputResampler::process`] call.

# Errors

Returns [`StageError::Unsupported`] when `native_rate` lies
outside [`MIN_NATIVE_RATE`] to [`MAX_NATIVE_RATE`] (inclusive), or
when its reduced ratio to the engine rate has a term above
[`MAX_RATIO_TERM`] (a rate no real device reports, which would
otherwise design an enormous filter bank).

Source: `crates/noican-core/src/capture.rs` lines 365–367.

In range but coprime (or nearly) with 48 000: the reduced ratio
would need tens of thousands of phases, so it is refused
before any filter is designed.

## native-rate-capture

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 8–15.

The device is fixed to a native rate other than 48 kHz —
Bluetooth telephony profiles (HFP/SCO at 8/16/24 kHz), the
44.1 kHz family (44.1/22.05/11.025 kHz), 88.2/96 kHz interfaces:
captured natively through the split transport and resampled to
48 kHz inside it by the exact rational ratio (issue #7). The
associated value is the snapshot's nominal rate in Hz; the
transport re-reads the live rate at start time (Bluetooth
profiles renegotiate).

## native-rate-range-sync

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 22–24.

Native rates the split transport's resampler converts, in Hz.
Mirrors `noican_core::capture::{MIN,MAX}_NATIVE_RATE` (the Rust
side re-validates at start); keep the two in sync.

## noican-driver-uid-compat

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift` lines 55–61.

Device-UID prefix of the Noican-branded driver (trailing dot
included). The driver build (lightsound/noican-driver) derives its
device UID "com.lightsound.noican.mic_UID" (driver 0.2.0, one
channel) from this prefix, as it did the 0.1.0 driver's
"com.lightsound.noican.2ch_UID"; both are recognized, so old and
new drivers can be swapped under one app build. Matching is
case-insensitive by lowercasing first.

## non-macos-portable-stub

Source: `crates/noican-coreaudio/src/lib.rs` lines 145–145.

Non-macOS placeholder that keeps workspace checks portable.

## sample-rate-fail-open

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift` lines 147–148.

Whether the device advertises support for `rate`. Returns true when
the supported-rate list is unreadable (let the actual set decide).

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift` lines 153–157.

Tri-state form of `supportsSampleRate`: whether the device's
advertised rate list contains `rate`, or nil when the list is
unreadable. Callers that have another source of truth (the
reducer's device snapshot) use the tri-state to fall back to it
instead of failing open onto the wrong transport path.

## untrusted-sample-rate-bounds

Source: `crates/noican-cli/src/audio.rs` lines 174–179.

The declared rate is unvalidated file data, and it sizes the
resampler's output buffer by `ENGINE_SAMPLE_RATE / sample_rate ×
input_len`: a header patched to a tiny rate (1 Hz would turn an
8 MB file into a ~768 GB allocation attempt) must be rejected
before the resampler is built. The accepted range covers every
real audio rate, from below telephony to DXD, with headroom.
