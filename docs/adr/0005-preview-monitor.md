# 0005. Preview monitor safety

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## feedback-trip-handling

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.monitorTripped`.

Reacts to the engine-side feedback killswitch: the worker already
silenced the preview, so release the playback device and tell the
user why. The mode stays on Preview (it is the user's intent);
the warning tint and the message say it is not playing, and
re-tapping Preview retries.

## howl-guard

Source: `crates/noican-coreaudio/src/monitor.rs`, `HowlGuard`.

Last-resort feedback killswitch for the preview monitor.

Trips when the teed output holds near clipping for
[`HOWL_TRIP_BLOCKS`] consecutive blocks — the signature of the preview
playing through speakers back into the microphone. Complements
[`classify_monitor_target`], which cannot classify every output.

Real-time safe: one fold over the observed block, no allocation, no
locks. Owned by [`MonitorTee`] on the inference worker.

Source: `crates/noican-coreaudio/src/monitor.rs`, `HowlGuard::observe`.

Observes one teed block. Returns `true` when the guard trips; the
caller must then stop feeding the monitor. The run resets on any
block below the threshold and after a trip.

## howl-guard-tuning

Source: `crates/noican-coreaudio/src/monitor.rs`, `HOWL_PEAK_THRESHOLD`.

Peak level (linear) a teed block must reach to count toward a trip.

Acoustic feedback has loop gain above one, so it grows until it
saturates near full scale; speech peaks touch this level but do not
hold it continuously.

Source: `crates/noican-coreaudio/src/monitor.rs`, `HOWL_TRIP_BLOCKS`.

Consecutive near-clipping blocks (10 ms each) before a trip: 500 ms.

Long enough that shouting into the microphone does not trip it
(speech dips between syllables), short enough to cut a howl before it
is painful.

## monitor-auhal-configuration

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `configure_monitor_auhal`.

Output-only AUHAL on the monitor device: input disabled, mono 48 kHz
engine samples rendered as interleaved stereo (AUHAL's converter
handles the device's own rate and format).

## monitor-lifecycle-and-ring

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorContext`.

Render context of the output-only monitor AUHAL. Owned by the monitor
callback while preview runs; reclaimed (with its ring consumer) when
preview stops so a later enable can reuse the preallocated ring.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl`.

Control-plane owner of the preview monitor. Invariants held here:
exactly one of `consumer` (parked while off) and `handle` (lent to the
running monitor AUHAL) is populated, and a feedback trip disarms the
tee (the shared [`MonitorState`] cell reads `Tripped`) while leaving
the AUHAL up until the next toggle.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `monitor_pair`.

Creates the two halves of the preview monitor around one preallocated
ring: the control half for [`super::Runtime`] and the worker half for
the inference thread. Resets any stale value left in `state` to
[`MonitorState::Off`].

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::enable`.

Starts (or re-arms) the monitor on the vetted system default
output.

# Errors

Returns a refusal from [`classify_monitor_target`] for unsafe
targets, and other [`CoreAudioError`] values when the monitor
AUHAL cannot start; the ring consumer is reclaimed on every error
path so a later enable can retry.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::enable`.

Discard anything left over from a previous preview session so
re-enabling never replays stale audio.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::enable`.

The AUHAL never came up, so the cell already reads
`Off`; park the consumer for a retry.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::disable`.

Stops the monitor AUHAL (if up) and parks the ring consumer for
the next enable. Idempotent.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::disable`.

Move the cell to `Off` first (also clearing a pending trip) so
the worker stops feeding the ring before the monitor AUHAL
goes away.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::disable`.

Callbacks have stopped; reclaim the ring consumer so a
later enable reuses the preallocated ring.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `start_monitor_unit`.

Creates, configures, and starts the output-only monitor AUHAL on
`device`, handing `consumer` to its render callback. On failure the
consumer is returned so the preallocated ring survives for a retry;
the partially configured unit is released by its RAII guard.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `start_monitor_unit`.

The unit never started, so no callback observed the context;
reclaim it to recover the ring consumer.

## monitor-module-split

Source: `crates/noican-coreaudio/src/macos/monitor.rs`.

Control-plane half of the preview monitor: resolving and vetting the
playback target, and owning the output-only monitor AUHAL's lifecycle.

The policy (which targets are refused) and the worker half (the
[`MonitorTee`]) live in the platform-independent [`crate::monitor`]
module so they stay unit-tested on every CI target; this module only
contains what needs Core Audio.

## monitor-preview-architecture

Source: `crates/noican-coreaudio/src/monitor.rs`.

Preview (self-monitor) policy and worker-side machinery.

Platform-independent by design so its invariants are unit-tested on
every CI target: the [`MonitorTee`] (the inference worker's half of the
monitor, including the [`HowlGuard`] feedback killswitch) and
[`classify_monitor_target`] (the safety decision about which output
devices may receive the preview). The macOS-only counterpart — the
monitor AUHAL lifecycle — lives in the `macos` transport module and
consumes both.

Real-time rules (docs/tech-research.md §9): [`MonitorTee::feed`] runs
on the inference worker between device callbacks — no allocation, no
locks; the gate is one plain atomic ([`MonitorState`]) and samples go
into a preallocated lock-free ring.

## monitor-priming-and-drift

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorContext::primed`.

Whether enough samples are buffered to play. Cleared on underrun so
playback resumes only after [`MONITOR_PRIME_SAMPLES`] accumulate,
turning clock drift into bounded silence gaps instead of crackle.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `monitor_render_callback`.

The engine is mono; every device channel gets the same sample. The
monitor counts no underruns: re-priming is its whole response.

## monitor-refusal-message-contract

Source: `crates/noican-coreaudio/src/lib.rs`, `CoreAudioError::MonitorLoopbackOutput`.

Enabling preview was refused because the system default output is a
virtual loopback device, which would feed the processed voice into
the meeting a second time.

The message is one short cause (no UID, no remedy): the UI composes
it into its own sentences ("Preview needs headphones — …",
"Preview stopped: …"), and the rejected device's UID is kept in
the variant for programmatic callers.

Source: `crates/noican-coreaudio/src/lib.rs`, `CoreAudioError::MonitorAggregateOutput`.

Enabling preview was refused because the system default output is
an aggregate or Multi-Output device, which can contain the meeting
loopback as a subdevice this check cannot cheaply inspect. Message
shape: see [`CoreAudioError::MonitorLoopbackOutput`].

Source: `crates/noican-coreaudio/src/lib.rs`, `CoreAudioError::MonitorSpeakerOutput`.

Enabling preview was refused because the system default output is
the built-in speakers, which would feed the processed microphone
straight back into itself (Phase 0/1 has no echo cancellation).
Message shape: see [`CoreAudioError::MonitorLoopbackOutput`].

## monitor-ring-latency-drift

Source: `crates/noican-coreaudio/src/monitor.rs`, `MONITOR_RING_CAPACITY`.

Monitor ring capacity: 100 ms at 48 kHz.

Deliberately small — the monitor AUHAL runs on the playback device's
own clock and drift is not corrected, so a slow drain pins the ring at
its capacity. A small ring caps the preview latency at ~100 ms and
turns the drift into an occasional discarded block instead (accepted
preview artifact).

Source: `crates/noican-coreaudio/src/monitor.rs`, `MONITOR_PRIME_SAMPLES`.

Samples the monitor ring must hold before playback (re)starts after an
underrun: 40 ms at 48 kHz. Priming turns scattered single-sample
underruns into one bounded silence gap.

## monitor-runtime-send-safety

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorHandle`.

Raw pointers of a running monitor AUHAL, stored as integers so the
runtime stays `Send`. They are only dereferenced on the control thread
that owns the runtime, after callbacks have been stopped.

## monitor-safety-listener-lifecycle

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift`, `AppState.syncMonitorSafetyObservation`.

Keeps the per-device data-source listener in lockstep with the
settled monitor state: registered on the monitor's device when an
enable settles, and always removed the moment the monitor is no
longer armed (disable claimed, trip, teardown) — a listener left
behind would fire for a monitor that no longer exists.

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift`, `AppState.syncMonitorSafetyObservation`.

Without the registration the listener is dead — do
not store it, and surface why the safety check
stopped reacting to jack reassignments.

## monitor-safety-observation

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.monitorSafetyListener`.

Follows the monitor's actual output device while the preview
plays: a data-source flip (headphone jack → internal speakers on
the same built-in device) fires no device-list or default-output
notification, so this per-device listener — plus the 1 Hz health
poll as belt and braces — is what catches it. Registered when a
monitor enable settles; always removed when the monitor disarms
(`syncMonitorSafetyObservation`).
Internal for `ListenerObservation.swift`, which owns its
registration lifecycle.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.registerDeviceListener`.

The monitor's target can vanish as a whole device (on
machines whose headphone jack is a separate device).

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.checkMonitorTrip`.

Forwards the engine-side feedback killswitch to the reducer, which
releases the playback device and explains why. Checked lock-free
at 20 Hz while the popover is open and at 1 Hz by the health poll.

## monitor-safety-recheck

Source: `macos/Sources/NoicanMenuBar/ListenerObservation.swift`, `AppState.checkMonitorSafety`.

Re-vets the device the running monitor actually plays on and
auto-stops the preview (via the reducer) when its safety is gone.
Two loss shapes exist, machine-dependent: the same built-in device
flips its data source from the headphone jack to the internal
speakers (caught by the per-device listener and the health poll),
or the jack is a separate device that disappears (caught by the
device-list listener). `noican_monitor_target_error` cannot serve
here: it judges the *current default output*, which may have moved
on while the monitor stayed on the old device.

## monitor-state-protocol

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::state`.

The shared [`MonitorState`] cell: this control half moves it
between `Off` and `Playing`; the worker's [`MonitorTee`] gates on
it and moves it to `Tripped`. Also read lock-free by the FFI
layer, which owns the `Arc` (`noican_engine_levels`).

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorControl::enable`.

The monitor AUHAL is still up (a feedback trip only disarms
the tee); re-arm it — which also clears the trip, since
`Playing` overwrites `Tripped` in the one shared cell.

Source: `crates/noican-coreaudio/src/monitor.rs`, `MonitorState`.

Preview monitor state: the single lock-free protocol value.

Shared by the control plane (which arms and disarms), the inference
worker (which trips), and the UI (which polls at 20 Hz). One
`AtomicI32` holds it, so no reader ever waits on the control mutex —
including while a slow monitor start holds it.

The values cross the C ABI verbatim
(`NoicanLevels::monitor`), so the discriminants are fixed.

Source: `crates/noican-coreaudio/src/monitor.rs`, `MonitorState::Off`.

No preview is playing: the engine is stopped, the preview was
never enabled, or a disable tore the monitor AUHAL down.

Source: `crates/noican-coreaudio/src/monitor.rs`, `MonitorState::Tripped`.

The feedback guard tripped: the monitor AUHAL is *still up* but
the tee is disarmed, so it renders silence. The next
`set_monitor` toggle in either direction clears this — enable
re-arms the same AUHAL back to [`MonitorState::Playing`], disable
tears it down to [`MonitorState::Off`].

Source: `crates/noican-coreaudio/src/monitor.rs`, `MonitorState::from_raw`.

Decodes a raw atomic/C-ABI value; anything unknown reads as
[`MonitorState::Off`] (the safe interpretation for a poller).

Source: `crates/noican-coreaudio/src/monitor.rs`, `MonitorTee::state`.

The shared [`MonitorState`] cell: armed/disarmed by the control
plane; moved to [`MonitorState::Tripped`] here on a trip.

Source: `crates/noican-coreaudio/src/monitor.rs`, `tests::monitor_state_round_trips_and_defaults_to_off`.

The C-ABI discriminants are frozen (noican.h).

Source: `crates/noican-coreaudio/src/monitor.rs`, `tests::rearming_after_a_trip_requires_a_fresh_sustained_run`.

Control plane re-arms (as Runtime::set_monitor(true) does).

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.setMonitor`.

Toggles the preview self-monitor (processed voice on the system
default output). Fails when the engine is stopped or the default
output is a virtual loopback; the meeting-facing path is never
affected. The monitor does not survive an engine stop/start, so
callers re-enable it after `start`.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanMonitorState`.

Preview monitor state as it crosses the C ABI in
`NoicanLevels::monitor`. The C enum in noican.h is generated from
this one and the app compares against the generated constants, so
the numbering has a single definition.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanMonitorState::Tripped`.

The feedback guard silenced the preview: the monitor AUHAL is
still up but renders silence. Cleared by the next monitor toggle
in either direction — enabling re-arms, disabling tears down.

## monitor-target-error-live-clearing

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.monitorTargetErrorChanged`.

Re-sample of the preview-target pre-flight while a refusal message
is showing, so the message clears (or updates) live as the user
fixes the default output.

## monitor-target-safety

Source: `crates/noican-coreaudio/src/monitor.rs`, `classify_monitor_target`.

Decides whether an output device may receive the preview.

Refused targets:
- **Virtual loopbacks** (`virt` transport or a `BlackHole`/Noican UID)
  — the processed voice would reach the meeting a second time.
- **Aggregate / Multi-Output devices** (`grup` transport) — they can
  contain the meeting loopback as a subdevice, which this check cannot
  cheaply inspect, and the feedback guard cannot catch that route
  (it is not an acoustic loop).
- **The built-in internal speakers** (`bltn` transport with the
  `ispk` data source) — the voice would feed straight back into the
  microphone (Phase 0/1 has no echo cancellation).

Everything else — Bluetooth, USB, HDMI, the built-in headphone jack,
or unreadable properties (`transport == 0`, `data_source == None`) —
fails open: those cannot be classified reliably, and the worker-side
[`HowlGuard`] is the safety net for acoustic feedback through them.

The UID predicate must stay aligned with the Swift picker's
`AudioDeviceCatalog.isNoicanVirtualDevice`
(macos/Sources/NoicanMenuBar/CoreAudioDevices.swift); the Rust side is
deliberately broader (any virtual/aggregate transport).

# Errors

Returns the matching [`CoreAudioError`] refusal for the cases above.

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.monitorTargetBecameUnsafe`.

The device the playing monitor targets lost its safety after
enable time (the headphone jack flipped to the internal speakers,
or the device disappeared): the preview must stop itself before
the unvetted output keeps playing. `reason` explains what
happened, in the same voice as the enable-time refusals.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.monitorTargetBecameUnsafe`.

The playing monitor's target lost its safety (headphone jack
unplugged into the internal speakers, or the device vanished):
stop the preview immediately, exactly like a feedback trip — the
engine keeps running, the mode keeps the user's intent, the
reason renders under the control, and re-tapping Preview retries
on the (new) vetted output.

## monitor-target-vetting

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `MonitorHandle::device`.

Device the AUHAL was opened on (the vetted default output at
enable time). Exposed to the control plane so it can watch *this*
device for losing its safety — the default output may move on
while the monitor stays here.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `check_monitor_target`.

Checks whether the current system default output may receive the
preview, without creating or changing any audio object.

A pure inspection (a few Core Audio property reads against the HAL's
in-process cache), cheap enough for the UI to call before every
enable and whenever the default output changes — so an unsafe target
can be surfaced *before* the user tries to preview into it.

# Errors

Returns exactly the refusals enabling would produce: the
[`classify_monitor_target`] matrix (loopback, aggregate, built-in
speakers) or [`CoreAudioError::Monitor`] when no default output is
configured.

Source: `crates/noican-coreaudio/src/macos/monitor.rs`, `check_monitor_device`.

Checks whether a *specific* output device may (still) receive the
preview, without creating or changing any audio object.

The same [`classify_monitor_target`] vetting as
[`check_monitor_target`], but against a caller-chosen device instead
of the current default output. The control plane runs it against the
device the running monitor actually plays on (see
[`super::Runtime::monitor_device`]): after enable time the default
output can move on while the monitor stays put, and — worse — a
built-in device can flip its data source from the headphone jack to
the internal speakers (`'hdpn'` → `'ispk'`) without any device-list
or default-output notification. Re-classifying the monitor's own
device is what catches that.

A vanished device is *not* reported here: its properties read as
unclassifiable, which fails open by policy. Device loss is visible in
the device list and is the caller's check.

# Errors

Returns the [`classify_monitor_target`] refusals (loopback, aggregate,
built-in speakers).

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.monitorTargetError`.

Reason the current system default output must not receive the
preview (loopback, aggregate, or built-in speakers), or nil when
preview may start. A pure inspection — a few Core Audio property
reads, no audio objects — so it is cheap to poll.

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.monitorDeviceError`.

Reason a *specific* device must not receive the preview — the same
vetting as `monitorTargetError`, applied to the device the monitor
actually plays on rather than the current default output (the two
diverge once the default output moves after enable time). Catches
the same-device data-source flip from the headphone jack to the
internal speakers; a vanished device reads as unclassifiable and
is the caller's device-list check instead.

## monitor-tee-overrun-and-trip

Source: `crates/noican-coreaudio/src/monitor.rs`, `MonitorTee::feed`.

Feeds one processed block into the monitor ring when the shared
state reads [`MonitorState::Playing`], without blocking.

Ring overrun (a monitor device draining slower than the engine
clock) silently discards the overflowing samples: preview
tolerates minor artifacts and must never push back on the
meeting-facing path. When the fed audio holds near clipping long
enough for the [`HowlGuard`] to trip, the tee moves the shared
state to [`MonitorState::Tripped`] — one store that both disarms
this tee and tells the control plane why. While disarmed the ring
is not touched at all. Returns whether the block was teed, so
both branches are observable in tests.

## monitoring-section-visibility

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.showsMonitoring`.

Whether the monitoring section (level meters) is shown: only for
a settled, healthy run, so the section neither slides in during a
start that may still fail nor disappears during a model switch.

## preview-monitor

Source: `crates/noican-coreaudio/src/pump.rs`, `Pump::run_block`.

Preview branch: the tee only copies into its preallocated monitor
ring (skipped entirely while disarmed) and disarms itself on
sustained feedback; it never delays the main path below.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::set_monitor`.

Enables or disables the preview self-monitor: the worker tees its
processed output into the monitor ring and a second, output-only
AUHAL plays it on the system default output device.

The monitor target is resolved at enable time; it does not follow a
later default-output change (re-enable preview to pick it up).
Monitor failures never affect the meeting-facing path, and both
directions are idempotent. Enabling clears a pending feedback trip
and re-arms a still-running monitor.

Enabling can take a while (`AudioOutputUnitStart` on a sleeping
output device); callers that also poll lock-guarded status getters
should gate concurrent control calls (the menu app uses its busy
flag for this).

# Errors

Returns [`CoreAudioError::NotRunning`] after [`Runtime::stop`], a
refusal from [`crate::monitor::classify_monitor_target`] when the
default output must not receive the preview (loopback, aggregate,
or built-in speakers), and other [`CoreAudioError`] values when the
monitor AUHAL cannot start.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::monitor_device`.

Device the running preview monitor plays on (resolved and vetted
at enable time), or `None` while the monitor is down. The control
plane re-vets this device with [`check_monitor_device`] while the
preview plays: the safety decision made at enable time can be
invalidated later (headphone jack unplugged → the same built-in
device flips to the internal speakers) without the monitor
noticing.

## preview-monitor-contract

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_set_monitor`.

Enables (nonzero) or disables (zero) the preview self-monitor: the
processed microphone signal plays on the system default output device.

The monitor target is resolved to the current default output at enable
time; re-enable preview to follow a later default-output change. (A
future explicit device selection would arrive as a separate setter,
keeping this signature stable.) The monitor does not survive an engine
stop/start, so callers re-enable it after `noican_engine_start`.

Enabling fails when the engine is not running, when the default output
must not receive the preview (a virtual loopback, an aggregate /
multi-output device, or the built-in speakers), or when the monitor
AUHAL cannot start; the meeting-facing path is never affected.
Disabling is always a success, including while stopped. Toggling holds
the control lock for the monitor start/stop transition — starting an
output device can take a moment, so callers should serialize their own
control calls behind a busy flag while a toggle is in flight (the
level and monitor-state getters stay lock-free and are always safe to
poll). A toggle in either direction clears a pending feedback trip
(see `NoicanLevels::monitor`).

# Safety

`handle` must be null or a live engine handle.

Source: `crates/noican-ffi/src/lib.rs`, `noican_monitor_target_error`.

Checks whether the current system default output may receive the
preview, without starting or changing anything.

Returns 0 when preview can target the device. Otherwise copies the
human-readable refusal reason as UTF-8 (loopback / aggregate /
built-in speakers — the same vetting enabling applies) and returns the
required byte count including the terminating NUL. Cheap (a few Core
Audio property reads): the UI calls it before enabling and whenever
the system default output changes, so an unsafe target disables the
Preview control up front instead of failing after the fact.

# Safety

A non-null `buffer` must be writable for `capacity` bytes.

Source: `crates/noican-ffi/src/lib.rs`, `noican_monitor_device_error`.

Checks whether a *specific* output device may (still) receive the
preview.

The same vetting as [`noican_monitor_target_error`], but against a
caller-chosen device instead of the current default output.
The UI runs it against the device the running monitor actually plays
on (see `NoicanHealth::monitor_device`): after enable time the
default output can move on while the monitor stays put, and a
built-in device can flip its data source from the headphone jack to
the internal speakers without any device-list or default-output
notification — re-classifying the monitor's own device is what
catches that. A vanished device is *not* reported here (unreadable
properties fail open by policy); device loss is visible in the device
list and is the caller's check.

Returns 0 when the device may receive the preview. Otherwise copies
the human-readable refusal reason as UTF-8 and returns the required
byte count including the terminating NUL.

# Safety

A non-null `buffer` must be writable for `capacity` bytes.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::monitor_device`.

Device ID the running preview monitor plays on, or 0 while no monitor
AUHAL is up (stopped engine, preview off).

The target is resolved on the Rust side at enable time and stays
fixed until the next toggle, so this is how the UI learns which
device to watch for losing its safety
(`noican_monitor_device_error`, device-list changes).

Reads the control mutex (never held across slow work), so it is part of
the health snapshot for event-driven and low-rate callers — not of
the 20 Hz levels snapshot.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanLevels::monitor`.

The preview monitor's state as one value.

0 = off (no monitor AUHAL, including a stopped engine and a null
handle), 1 = playing, 2 = tripped. The protocol lives in the
[`MonitorState`] enum shared with the engine — including that
*tripped* means the monitor AUHAL is still up but silenced (the
feedback guard disarmed the tee), and that the next
`noican_engine_set_monitor` call in either direction clears it
(enable re-arms, disable tears down).

On 2, callers should disable the monitor to release the playback
device and tell the user why; the meeting-facing path is unaffected.

Reads one atomic without taking the control lock, so it never blocks —
safe to poll at UI rates even while a monitor start is in progress.

Source: `crates/noican-ffi/src/lib.rs`, `tests::monitor_target_check_is_null_safe_and_consistent`.

The outcome depends on the host's audio devices (and on Linux it
is always a refusal), but the contract holds everywhere: a null
buffer only measures, and a nonzero result re-reads as a
NUL-terminated reason string.

Source: `crates/noican-ffi/src/lib.rs`, `tests::monitor_device_check_is_null_safe_and_consistent`.

Same contract as the default-output check: a null buffer only
measures, and a nonzero result re-reads as a NUL-terminated
reason string. Device 0 (kAudioObjectUnknown) has unreadable
properties, which fail open on macOS by policy.

## preview-monitor-isolation

Source: `crates/noican-coreaudio/src/lib.rs`, `CoreAudioError::Monitor`.

The preview monitor path could not start. Monitor failures never
affect the meeting-facing path.

## preview-monitor-sequencing

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.startCompleted`.

Preview mode is engine + monitor; the monitor half
starts once the transport is up, staying busy (and
rendering the same snapshot) until its outcome lands.

## preview-refusal-live-clearing

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.refreshPreviewAvailability`.

Maintains a shown refusal message only: once a Preview attempt was
refused, re-sample the pre-flight so the reducer clears (or
updates) the message live as the user fixes the output. Called
from the default-output listener, device hot-plug, and popover
polling.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.registerDefaultOutputListener`.

Follows default-output changes so a shown refusal message clears
the moment the user switches to a safe device.

## preview-refusal-message

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.modeSelected`.

Refuse in place and explain: neither the mode nor the
engine changes, and `monitorTargetErrorChanged` clears the
message as soon as the output becomes safe.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.monitorTargetErrorChanged`.

Maintains a shown refusal message only: once a Preview attempt was
refused, the shell re-samples the pre-flight so the message clears
(or updates) live as the user fixes the default output.

## preview-refusal-message-shape

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.modePicker`.

One short sentence: the engine reports the cause (no
UID, no remedy); the remedy is always the same.
