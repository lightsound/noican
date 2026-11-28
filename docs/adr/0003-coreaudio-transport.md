# 0003. CoreAudio transport architecture

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## audio-stall-heartbeat

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.checkEngineHealth`.

Heartbeat: the device must keep delivering callbacks while
running. Three silent seconds means it stopped (unplugged mic,
coreaudiod restart, post-sleep stall) without reporting an error.

## auhal-channel-map

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::routing_description`.

Diagnostic: how the render stream is routed into the device. On
the aggregate transport: the output channel count AUHAL reports for
the aggregate, the channel map requested, and the map read back
after `AudioUnitInitialize` (the point where AUHAL reconciles its
configuration with the device; whether it may alter a map there is
not documented, which is why the read-back is recorded rather
than assumed). On the split transport: the output channel count
AUHAL reports for the virtual output device, the render format
set from it, and that format read back after initialize (see the
`split` module). Meant for the one-time start log on hardware, where
neither the map's effect nor the format negotiation can otherwise
be observed. The two transports' descriptions start differently
(`aggregate output channels …` / `virtual output channels …`).

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `configure_auhal`.

Dual mono: as many client channels as the virtual output has, so a
one-to-one channel map can feed every one of them (crate::routing).
A 1-channel virtual output yields the mono format of old.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `set_render_channel_map`.

Places the render stream (one client channel per virtual-output
channel) on the virtual output's channels inside the aggregate device
(see [`crate::routing`] for the bug and the decision).

The recipe follows Apple's AUHAL notes ("Channel Maps", reproduced
verbatim in `PortAudio`'s `src/hostapi/coreaudio/notes.txt`): for
output, the map is an `SInt32` array sized by the device's channel
count — "Get the Format of the AUHAL's output Element == 0" — with
every entry `-1` except `map[deviceOutputChannel] = clientChannel`,
each client channel named once.
That device-side format is read from `kAudioUnitScope_Output`,
element 0 (the only place it lives; TN2091 notes it is never
writable, so the client format set on the input scope cannot leak
into the read). The map is set on that same scope and element:
the property is documented for the input and output scopes, and
`PortAudio`'s Core Audio host API (`pa_mac_core.c`) — the shipping
implementation this was checked against — sets the output-element
map on the output scope, after the stream formats and before
`AudioUnitInitialize`, which is the order used here. AUHAL's default
map (identity: client `i` → device `i`) is what sent the engine
output into a headphone-equipped microphone's own outputs.

The capture direction keeps AUHAL's default map (client channel 0 ←
device input channel 0 = the microphone, which is the first
subdevice); it is deliberately not touched here.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `describe_applied_channel_map`.

Reads the output channel map back for the start-time diagnostics.
Called after `AudioUnitInitialize`, the point where AUHAL reconciles
its configuration with the device: before it, a read would only echo
the array the setter just accepted. On hardware the map's effect is
otherwise invisible, so this — together with the requested map — is
the first thing to look at when the virtual microphone stays silent.
Never fails the start: an unreadable map is reported as such.

## auhal-direction-scopes

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Direction`.

One of AUHAL's two I/O directions. Each has its own element (bus),
split into a device-facing and an app-facing scope: capture enters
the input element from the device on its input scope and reaches the
app on its output scope; render enters the output element from the
app on its input scope and reaches the device on its output scope
(Apple TN2091). The setup helpers below take a direction, so each
unit's configuration names what it switches or formats instead of
spelling out that scope and element.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `stream_format_channels`.

Channel count of the output element's stream format on `scope`:
[`AUDIO_UNIT_SCOPE_OUTPUT`] is the device side (what the hardware
carries), [`AUDIO_UNIT_SCOPE_INPUT`] the client side (the render
format this transport set, as AUHAL holds it).

## capture-backlog-shedding

Decided 2026-11-28; recorded here because code comments are banned.

On the aggregate transport capture and render run in one callback
at one rate, so whatever the worker falls behind by never drains:
after a stall it works through the queued input and the same amount
sits in the output ring for good. Measured with the real feeds, the
microphone delay went from 14.7 ms to 208 ms after one 200 ms stall
and was unchanged a minute later. The worker therefore checks the
capture ring at each block boundary and, once more than
`MAX_CAPTURE_BACKLOG` samples wait (the largest callback plus two
worker blocks, about 105 ms), discards all but the newest block.
The listener already heard silence for the stalled span, so the
discarded audio is the part that could only have arrived late.
Shorter stalls are left alone on purpose: shedding them would turn
a delay nobody notices into an audible gap, so the delay can still
grow to the limit but no further, and a long stall resets it.
Discarded samples are added to the input-overrun counter, which
already means "captured audio that never reached the models". The
split transport needs none of this; its drift servo pulls the
output ring back to its target.

## capture-error-contract

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Capture::capture`.

Captures this cycle's `frame_count` frames and wakes the worker.

On failure the error is the status the callback returns: `paramErr`
for a cycle larger than the landing buffer (cannot happen while
`kAudioUnitProperty_MaximumFramesPerSlice` holds; the callback never
allocates to compensate; the fault flag is set), `noErr` for a failed
`AudioUnitRender`. Since 2026-11-28 a failed render sets the fault
flag only once failures persist, see `render-failure-tolerance`; its
frames are added to the input-overrun counter either way.

# Safety

`action_flags` and `timestamp` must be the arguments AUHAL passed
to the running callback.

## capture-landing-buffer

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Capture::buffer`.

Preallocated landing buffer for `AudioUnitRender` (mono,
[`MAX_CALLBACK_FRAMES`]). On the aggregate transport the render
buffer AUHAL hands over carries one channel per virtual-output
channel, so it cannot double as the mono capture target.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `set_max_frames_per_slice`.

Bounds the unit's callback size to [`MAX_CALLBACK_FRAMES`] so the
preallocated capture landing buffer always suffices (the callback
never allocates; see [`crate::callback`]).

## render-failure-tolerance

Source: `crates/noican-coreaudio/src/callback.rs`, `RenderFailures`.

Decided 2026-11-28; recorded here because code comments are banned.

A failed `AudioUnitRender` in the capture path used to set the fault
flag at once, and the flag never clears: one failed cycle left the
app showing "Audio fault" for the rest of the session although audio
kept flowing from the next cycle on. The flag exists to tell the
user that the microphone path is broken, and a single lost cycle is
a dropout, not a broken path. Failed cycles are therefore counted by
the frames they cover, in a run that any successful cycle ends, and
the fault is flagged only when one run reaches 100 ms of capture
(4800 frames at 48 kHz, 1600 at a 16 kHz telephony microphone). The
tolerance is measured in audio time, not in cycles, because one
cycle is anything from 3 ms to 85 ms depending on the device buffer.
100 ms matches the stall the aggregate worker absorbs before it
sheds (see `capture-backlog-shedding`): shorter gaps are treated as
dropouts on both sides of the capture ring. A render that keeps
failing still faults within 100 ms, well inside the one-second
health poll that reports it. The frames of every failed cycle,
tolerated or not, are added to the input-overrun counter ("captured
audio that never reached the models"), so a tolerated failure still
leaves a trace in the log. Clearing the flag after a success instead
was rejected: the app reads it once a second and would miss a fault
that came and went, and a session it already marked failed would
not recover anyway. Every other fault source stays immediate.

## capture-non-finite-samples

Decided 2026-11-27; recorded here because code comments are banned.

`CaptureFeed::push` queues 0.0 in place of a NaN or infinite sample.
One non-finite sample is enough to poison the recurrent state of
dfn3, dpdfnet2, dpdfnet8, hush and hush-48k for the rest of the
session: every later output sample is NaN, no stage reports an
error, and the fault flag stays clear, so the virtual microphone
carries NaN under a green status. The capture feed is the one
place every captured sample of either transport passes before any
stateful DSP (the split transport's resampler included), so the
check lives there rather than in each stage. It costs one
comparison per sample on the callback thread and neither allocates
nor blocks. Files the CLI reads do not pass through it.

## capture-ring-overrun-accounting

Source: `crates/noican-coreaudio/src/callback.rs`, `CaptureFeed`.

The capture side's hand-off to the inference worker: pushes captured
samples into the capture ring, counting the ones a full ring drops.

A full ring means the worker (or the capture side) fell behind by more
than the whole ring, so those samples are dropped on the floor rather
than blocking the callback. A count that moves means real audio was
silently lost — audible as clipped input reaching the models — which
the frame heartbeat alone cannot show.

Source: `crates/noican-coreaudio/src/callback.rs`, `CaptureFeed::push`.

Queues `samples` for the worker. Never blocks; each dropped sample
is counted with a relaxed add (real-time safe).

## capture-support-classification

Source: `macos/NoicanState/Tests/NoicanStateTests/NativeRateReducerTests.swift`, `CaptureSupportTests.nativeRates`.

A device idling exactly at 48 kHz without advertising it in
its rate list is convertible too (1/1 through the same
resampler); the live probe still prefers the aggregate path.

## capture-support-in-state

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift`, `CaptureSupport`.

How a microphone can feed the 48 kHz engine, decided from the Core
Audio snapshot at device-refresh time. Carrying this here lets the
reducer pre-flight selections without touching Core Audio.

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift`, `CaptureSupport.engineRate`.

The device can run at the 48 kHz engine rate: captured through
the private Aggregate Device (the original, unchanged path).

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift`, `CaptureSupport.classify`.

Classifies a device from the two Core Audio facts the shell
samples: whether it advertises 48 kHz support, and its current
nominal rate. Pure, so the policy stays reducer-testable.

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift`, `InputDevice`.

One selectable microphone as the state machine sees it: a pure value
snapshot taken by the control plane at device-refresh time. Carrying
the capture capability here lets the reducer pre-flight selections
without touching Core Audio (the list itself follows hot-plug via
`AppEvent.devicesChanged`; a running device's live rate change is
observed separately via `AppEvent.inputSampleRateChanged`).

## coreaudio-generic-pointer-read

Source: `macos/Sources/NoicanMenuBar/VirtualOutputLevelCheck.swift`, `VirtualOutputLevelProbe.readProperty`.

An explicit typed pointer: `&value` on a generic inout would
form a raw pointer to a possibly reference-holding `T`,
which the compiler (rightly) warns about.

## coreaudio-module-layout

Source: `crates/noican-coreaudio/src/macos/mod.rs`.

Audited AUHAL FFI and real-time callbacks for the main transport.
The worker loop itself is portable and lives outside this module
tree (see `worker-pump`).

The preview monitor's AUHAL lifecycle lives in the [`monitor`]
submodule; shared plumbing (unit creation, property setting, render
callback attachment, disposal) is defined here and reused there.

## coreaudio-transport-and-source-codes

Source: `crates/noican-coreaudio/src/monitor.rs`, `TRANSPORT_TYPE_VIRTUAL`.

`kAudioDeviceTransportTypeVirtual`: loopback drivers (`BlackHole`,
JoyCast, ...).

Source: `crates/noican-coreaudio/src/monitor.rs`, `TRANSPORT_TYPE_AGGREGATE`.

`kAudioDeviceTransportTypeAggregate`: aggregate and Multi-Output
devices composed in Audio MIDI Setup.

Source: `crates/noican-coreaudio/src/monitor.rs`, `TRANSPORT_TYPE_BUILT_IN`.

`kAudioDeviceTransportTypeBuiltIn`: the Mac's own output.

Source: `crates/noican-coreaudio/src/monitor.rs`, `DATA_SOURCE_INTERNAL_SPEAKER`.

Built-in output data source for the internal speakers
(`kAudioDevicePropertyDataSource` value `'ispk'`; the headphone jack
reports `'hdpn'`).

## coreaudio-transport-architecture

Source: `crates/noican-coreaudio/src/lib.rs`.

AUHAL transport between Core Audio devices and the Rust engine.

Ported from the Phase 0 transport candidate that passed hardware
acceptance on macOS 26 / Apple Silicon (mic permission → Running,
QuickTime recording through `BlackHole` 2ch, clean teardown), rewired to
drive [`noican_core::SwitchingEngine`] so the processing path — including
the 16 kHz polyphase resampling in [`noican_core::FramedStage`] — is the
exact code the CLI comparison mode exercises.

Two transport shapes exist: one AUHAL on a private Aggregate Device
for 48 kHz-capable microphones (the original path, unchanged), and a
split transport — a native-rate capture AUHAL plus a 48 kHz virtual-
output AUHAL bridged by a drift-compensating resampler — for
telephony-profile microphones such as Bluetooth headsets (issue #7).
On the aggregate path the engine output is rendered as one client
channel per virtual-output channel (the mono engine sample duplicated
into each — dual mono, like the split path and the monitor) and
steered to the virtual output's channels with an explicit one-to-one
AUHAL channel map ([`routing`]), so a microphone with its own output
channels (a headphone jack) does not capture the engine output ahead
of the virtual output.

Real-time rules (docs/tech-research.md §9): the Core Audio render
callback only calls `AudioUnitRender`, moves `f32` samples through
preallocated lock-free SPSC rings, and signals a dispatch semaphore
(non-blocking); inference runs on a dedicated worker thread joined to
the device's `os_workgroup`, blocking on that semaphore between
callbacks instead of spinning; output-ring underrun produces silence,
never blocking the callback.

## heartbeat-frame-counter

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Capture::frames`.

Heartbeat: input frames delivered since start (relaxed atomic add is
real-time safe). A counter that stops advancing while "running"
means the device stopped calling back (unplugged microphone,
coreaudiod restart, post-sleep stall).

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::frames_processed`.

Total input frames the render callback has delivered since start.
A value that stops advancing while running means the device stopped
calling back; the control plane uses it as a heartbeat.

## realtime-callback-frame-cap

Source: `crates/noican-coreaudio/src/callback.rs`, `MAX_CALLBACK_FRAMES`.

Largest callback an AUHAL unit may deliver, in frames. Set as
`kAudioUnitProperty_MaximumFramesPerSlice` on every unit that
captures, and the length of each preallocated capture landing buffer.

Source: `crates/noican-coreaudio/src/callback.rs`, `capture_byte_size`.

Byte size of a mono `f32` capture landing buffer holding `frames`,
or `None` when `frames` exceeds `capacity` (the buffer's length) —
the callback must then fault instead of rendering past the buffer.

## realtime-callback-rules

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `render_samples`.

The interleaved `f32` samples a render callback may write — the first
buffer of the list AUHAL handed over, bounded by [`render_geometry`] —
with their channel count, or `None` for a malformed list (no buffers,
or a null data pointer). Shared by the three render callbacks
(aggregate, split output, preview monitor).

# Safety

`data` must be the non-null buffer list AUHAL passed to the running
render callback; the slice must not outlive that callback.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `render_callback`.

Render callback of the aggregate AUHAL: captures the microphone frames
of this cycle into the preallocated mono landing buffer, pushes them to
the input ring, and fills the (one channel per virtual-output channel)
render buffer from the output ring, duplicating each engine sample
into every channel of its frame — the same dual-mono shape the split
transport's output callback and the preview monitor produce.
Real-time rules (docs/tech-research.md §9): no allocation, no locks;
a callback larger than the frame bound faults instead of allocating;
ring overrun drops samples; underrun renders silence.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `render_callback`.

The render buffer is silenced so nothing stale can reach the
virtual output should AUHAL play it despite the error.

## render-geometry-zero-channel

Source: `crates/noican-coreaudio/src/callback.rs`, `render_geometry`.

Bounds a render request to the buffer AUHAL handed over.

`frame_count` frames of `number_channels` interleaved `f32` channels,
never more than `data_byte_size` bytes. A zero channel count is treated
as mono so a malformed buffer still yields silence rather than a
division by zero.

## setup-guard-ownership

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `ContextGuard`.

Owns a heap-allocated callback context during setup, reclaiming it
on any early error return (safe because the audio unit is never started
on those paths, so no callback can observe the pointer).

## split-cushion-experiment

Decided 2026-11-28; recorded here because code comments are banned.

Source: `crates/noican-coreaudio/src/cushion.rs`, `requested`.

Whether the split transport can also serve 48 kHz microphones, and
with how little added delay, can only be measured on hardware, and
hardware sessions are scarce. One environment variable,
`NOICAN_SPLIT_CUSHION_MS`, turns a single session into a sweep
without a rebuild: when it is set, every microphone takes the split
transport and the output cushion is that many milliseconds; when it
is unset nothing changes. The variable carries both the switch and
the value so the two cannot disagree. It is read once per start, on
the starting thread. A value that is not a whole number from 1 to
500 refuses the start by name instead of falling back, because a
silent fallback would attribute a measurement to the wrong cushion.
This is an experiment switch, not a setting: remove it with the
decision on the aggregate transport.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.shouldUseAggregate`.

The Swift side only tests whether the variable is present, before
any device fact is read; the value is parsed in one place, in Rust.
A present but malformed value therefore reaches the split start and
is refused there.

Source: `crates/noican-coreaudio/src/cushion.rs`, `SHIPPED_SAMPLES`.

The cushion every split start uses when the variable is unset:
50 ms, chosen for Bluetooth burst jitter and accepted on hardware.
It is added latency of the split path only.

Source: `crates/noican-coreaudio/src/cushion.rs`, `phase_estimate`.

A rough estimate of the cushion a split session needs before any
scheduling jitter: one capture burst (in engine samples), one
render buffer and one block. The reasoning behind it: the worker
can only push whole 480-sample blocks, and only when a capture
callback delivers; the render callback takes a whole buffer at
once; between two pushes up to a burst plus a block less a sample
can be consumed, and the render that follows still needs a full
buffer; two free-running clocks drift through every relative
phase, so an unlucky alignment always comes around.

It is an estimate, not a bound. The servo regulates a smoothed
total (output ring, pending samples and queued input), so the
ring's level right after a push moves independently of the target,
and the largest residue and the longest wait need not coincide.
The simulator's smallest gap-free cushion (table below, 240-sample
grid) falls between about 110 samples under the estimate and 350
over it: under it for 44.1 kHz at 512 frames (1,440 against 1,550)
and 96 kHz at 512 frames (1,200 against 1,248), over it elsewhere.
The servo's proportional residual (0.48 samples per ppm of drift)
and real scheduling jitter come on top. A hardware sweep must not
use the estimate to rule a cushion out.

The aggregate transport needs none of this because capture and
render are the same callback: only the 480-against-buffer-size
quantization remains, which the simulator settles at 448 samples.
For a 48 kHz microphone with 512-frame buffers the simulator clips
at 1,440 samples and is gap-free at 1,680, so the split path costs
roughly 21 to 26 ms more delay than the aggregate path at that
buffer size, before any jitter margin. Smaller device buffers
lower it (gap-free at 960 samples with 128 frames).

Source: `crates/noican-coreaudio/src/cushion.rs`, `describe`.

The routing line the app logs at start carries the cushion, both
device buffer sizes and the estimate, so a hardware run records
which cushion it measured and how that cushion relates to the
estimate. An unreadable buffer size is reported as such and leaves
the estimate unknown; it never fails the start.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `buffer_frames`.

AUHAL forwards the device's I/O buffer size as a global-scope
property of the unit bound to it. Read after the device is
selected; any failure reads as absent.

Source: `crates/noican-coreaudio/src/pump.rs`, `tests::gaps_over_forty_seconds`.

The simulator, at ±300 ppm over 40 simulated seconds, with ideal
scheduling and capture callbacks placed right before the render
they precede; a different callback schedule can move these numbers. Smallest gap-free cushion on a 240-sample grid, by
capture rate and buffer size: 48 kHz at 512 frames 1,680; at 256
frames 1,200; at 128 frames 960; 44.1 kHz at 512 frames 1,440;
96 kHz at 512 frames 1,200; 24 kHz at 512 frames 2,160; 16 kHz at
256 frames 1,680; 16 kHz at 512 frames 2,880. The last one is
above the shipped 2,400, where the simulator clips 32 samples per
pass through the worst phase: if a Bluetooth telephony device
really delivers 512-frame callbacks at 16 kHz, the shipped cushion
may be too small. Which buffer size it delivers is a hardware fact
the routing line now shows.

## split-transport-architecture

Source: `crates/noican-coreaudio/src/macos/split.rs`.

Split transport for non-48 kHz microphones (issue #7): two AUHAL
instances bridged by a drift-compensating worker.

An Aggregate Device drives all its subdevices at one nominal rate, and
the Noican virtual output only supports 44.1/48 kHz, so a microphone
that cannot run at 48 kHz — a telephony-profile Bluetooth headset
(HFP at 8/16/24 kHz), a 44.1 kHz-family device (44.1/22.05/
11.025 kHz), or a high-rate-only interface (88.2/96 kHz) — cannot
share an aggregate with the 48 kHz virtual output. This module
instead opens:

- an **input-only AUHAL** on the microphone device with a client
  format at its native rate (AUHAL performs no sample-rate conversion
  on the input side, so the client rate must equal the device rate);
- an **output-only AUHAL** on the virtual output device at the 48 kHz
  engine rate, with one client channel per virtual-output channel
  (the count is read from the device once the unit is bound to it —
  [`crate::routing::split_render_channels`]) and the mono engine
  sample duplicated into every one of them (dual mono, like the
  aggregate path and the preview monitor).

The two units run on separate device clocks. The inference worker
bridges them: it drains the native-rate capture ring, converts to
48 kHz through [`InputResampler`] (one arbitrary-ratio polyphase FIR
whose fractional phase step carries the drift correction — exact
160/147 for 44.1 kHz, 3/1 for 16 kHz), feeds the unchanged engine in
10 ms blocks, and pushes the result into the output ring. A
[`DriftServo`] observes the total samples buffered between the two
clock domains once per block — native-ring occupancy converted to
engine samples through the same ratio — and steers the resampler a
few hundred ppm to cancel clock drift (docs/tech-research.md §4.2).
The output ring is primed with a cushion of silence (see
`split-cushion-experiment`), which is both the initial jitter
cushion and the servo's occupancy target; the servo does not depend
on the ratio.

Real-time rules (docs/tech-research.md §9) hold as on the aggregate
path: the capture callback only calls `AudioUnitRender` into a
preallocated buffer, pushes into a lock-free ring, and signals the
worker's semaphore; the output callback only pops the output ring
(silence on underrun); all resampling runs on the worker with state
preallocated at start.

Source: `crates/noican-coreaudio/src/pump.rs`, `NATIVE_CHUNK_SAMPLES`.

Native-rate samples drained from the capture ring per worker pass
(also the resampler's preallocation unit): 480 native samples cover
20–60 ms of telephony-profile audio, comfortably above Bluetooth
burst sizes, and ≈11 ms at 44.1 kHz (about one engine block per
pass). The resampler's per-call output bound
([`InputResampler::max_output_len`]) sizes the worker's conversion
buffers, so a 96 kHz device (half an engine block per pass) needs no
separate tuning.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `OutputContext::feed`.

The output ring and its underrun counter. On this transport the
ring is primed with the cushion, so the feed counts from the first
callback on and counts every callback it cannot fill completely
(see `output-underrun-counting`): a short callback means the worker
(or the capture side) fell behind by more than the whole cushion.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `start`.

Validate the rate (and preallocate the conversion state) before
touching any audio object.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `start`.

Start the units, unwinding everything on failure (the RAII guards
release the units and contexts; the worker is stopped explicitly).

Source: `crates/noican-coreaudio/src/macos/split.rs`, `configure_capture_auhal`.

Input-only AUHAL on the microphone: output disabled, capture client
format at the device's native rate (AUHAL performs no sample-rate
conversion on the input side, so the client rate must match).

Source: `crates/noican-coreaudio/src/macos/split.rs`, `attach_input_callback`.

Registers `context` as the capture unit's input callback (the
input-only AUHAL notification slot, distinct from the render-callback
slot used by output units).

Source: `crates/noican-coreaudio/src/pump.rs`, `ClockBridge::new`.

Converted samples per pass, bounded by the resampler at the
largest drift correction so the buffer never grows afterwards.

## split-transport-contract

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::start_native`.

Opens the split transport for a microphone that cannot run at the
48 kHz engine rate (issue #7): an input-only AUHAL captures
`input_device` at `capture_rate` Hz (its native rate — any rate
from 8 to 192 kHz, see [`noican_core::capture`]), and an output-only
AUHAL feeds `output_device` (the `BlackHole`/Noican virtual
output) at 48 kHz. The inference worker bridges the two clock
domains, converting to the engine rate with a drift-compensating
resampler steered by ring occupancy
([`noican_core::capture`]).

No Aggregate Device is involved: an aggregate drives all
subdevices at one rate, so it cannot hold a 16 kHz microphone and
the 48 kHz virtual output at the same time.

`levels` and `monitor_state` behave exactly as in
[`Runtime::start`].

# Errors

Returns [`CoreAudioError`] when `capture_rate` lies outside the
resampler's range, or when AUHAL setup or worker startup fails —
including [`CoreAudioError::OutputRouting`] when `output_device`
reports no output channels (the render format is sized from that
count, see [`crate::routing::split_render_channels`]). Every error
path releases both AUHAL instances, the callback contexts, and the
worker.

## split-transport-native-capture

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_start_native`.

Starts the split transport for a microphone that cannot run at the
48 kHz engine rate (issue #7).

`input_device` is the microphone's Core Audio device ID and
`capture_sample_rate` its current nominal rate in Hz — whole hertz
from 8000 to 192000 whose ratio to 48000 reduces to small terms, i.e.
the standard rate families (Bluetooth telephony profiles at
8/16/24 kHz, the 44.1 kHz family, 88.2/96 kHz interfaces; see
[`noican_core::capture`]). `output_device` is the Noican/`BlackHole`
virtual output; the transport renders one client channel per output
channel that device reports (read on the Rust side once the output
AUHAL is bound to it — no channel count crosses this ABI) and refuses
to start when it reports none. No Aggregate Device is involved: the
microphone is captured natively and resampled to 48 kHz inside the
transport by the exact rational ratio, with clock drift between the
two devices compensated by a ring-occupancy servo.
The split path adds a 50 ms output cushion; the aggregate path
([`noican_engine_start`]) is unchanged.

Missing model weights are downloaded first (on this control thread,
without holding the control lock).

# Safety

`handle` must be a live engine handle and `model_id` must point to a
valid NUL-terminated string for the duration of this call.

## split-transport-render-format

Source: `crates/noican-coreaudio/src/routing.rs`, `split_render_channels`.

Client render channel count for the split transport's output-only
AUHAL, from the output channel count AUHAL reports for the virtual
output device it is bound to.

One client channel per device channel (dual mono — the callback
writes the engine sample into every one), so AUHAL's identity map
feeds the whole device whatever its width (see the module docs, "The
split transport's render format").

# Errors

Returns [`CoreAudioError::OutputRouting`] when the device reports no
output channels: a virtual output without output channels cannot
receive audio, and guessing a width (the old constant 2) would either
be refused by AUHAL or misdescribe the device — the transport refuses
to start instead.

## transport-device-binding

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineSession.inputUID`.

Microphone the running transport was built around (the aggregate
is composed at start time, so a live change means a rebuild).

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `StartAttempt.inputUID`.

Microphone to compose the aggregate around. Captured at claim
time: the *selection* can be reassigned by device hot-plug during
the busy window, but the transport is bound to this device.

## transport-pointer-ownership

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Transport`.

The AUHAL instance(s) a running transport owns, with their callback
contexts. Raw pointers are stored as integers so the runtime stays
`Send`; they are only dereferenced on the control thread that owns
the runtime, after callbacks have been stopped.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Transport::dispose`.

Disposes the audio unit(s) and reclaims the callback context(s).

# Safety

Callbacks must no longer be running (units stopped, worker
joined), and this must be called at most once.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::stop`.

Units are stopped and the worker joined, and `running` guards
against a second call.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `dispose_unit`.

Uninitializes and disposes a stopped audio unit. Callbacks must no
longer be running (the unit was stopped, and for the main transport
the worker joined).

## transport-session-accessors

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.liveSession`.

The live transport in a settled state: running, or failed with the
transport still up (an engine fault). Nil while busy — the busy
transition owns its session and no new transition may start.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.hasLiveTransport`.

Whether an engine transport exists that the health poll must
watch. True through busy monitor/model transitions (the transport
stays up) and through a fault (failed with a live session); false
while off, torn down, or rebuilding (`starting` claims a fresh
transport whose watch begins on success).

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `AppModel.transportSession`.

The session whose engine transport is currently up, *including*
through busy monitor/model transitions (which keep the transport
running) and through a fault that left it up. This is what
per-device observers must key on — the input-rate listener keyed
on `liveSession` would detach for the length of every Preview
toggle or model switch and miss a profile flip in that window.
Nil while off, torn down, or rebuilding (`starting` claims a
fresh transport).

## transport-session-listener

Source: `macos/NoicanState/Tests/NoicanStateTests/NativeRateReducerTests.swift`, `InputRateChangeTests.transportSessionSurvivesBusyTransitions`.

The input-rate listener keys on this: dropping it during a
Preview toggle or model switch would blind the observer while
the transport keeps running.

## transport-shape-decision

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.startEngine`.

Snapshot capability, kept as the fallback vote for the
transport-shape decision below.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.shouldUseAggregate`.

Transport shape for a start effect: 48 kHz-capable microphones
keep the original aggregate path unchanged; other devices
(Bluetooth telephony profiles, 44.1 kHz-family devices) are
captured natively through the split transport and resampled
inside it by the exact ratio (issue #7). Decided from
*live* facts at effect time — a rate-change rebuild can race a
profile flip, and a device that meanwhile reached a
48 kHz-capable state must take the aggregate path instead of
failing rate validation.

When the advertised-rate list is unreadable (the tri-state probe
reads nil), the live *nominal rate* — readable independently —
outranks the reducer's possibly stale snapshot: a device sitting
at a rate the split transport cannot convert (outside the
resampler's 8–192 kHz range) goes to the aggregate path, whose
`ensure48k` produces the precise refusal. Only for a live
convertible (or unreadable) nominal does the snapshot decide: a
`.nativeRate` pre-flight takes the split path — failing open onto
the aggregate would poll `ensure48k` for two seconds and then
fail a device the reducer already approved for native capture —
while unknown or non-native snapshots keep Phase 0's fail-open
behavior (a device idling at 48 kHz with an unreadable rate list
therefore still reaches `ensure48k`, which returns immediately).

## worker-pump

Decided 2026-11-28; recorded here because code comments are banned.

Source: `crates/noican-coreaudio/src/pump.rs`, `Pump`.

Both transports used to carry their own worker loop, written against
macOS types, so neither loop could run anywhere but on a Mac with
audio hardware, and every timing question (a clock that drifts, a
capture that drops out, an output device that goes silent) could
only be answered in a hardware session. The loop is now one portable
type that owns the worker's buffers and the engine, and one method,
`Pump::step`, that drains at most one chunk from the capture ring
and runs every block that chunk completes. It reports whether it
found anything, which is all the macOS thread needs to decide
between calling it again and sleeping on the semaphore. Thread
promotion, the workgroup and the semaphore stay in
`crates/noican-coreaudio/src/macos/mod.rs`; nothing in the pump
names a macOS type.

Source: `crates/noican-coreaudio/src/pump.rs`, `ClockBridge`.

The two transports differ in one fact: whether capture and render
share a clock. That fact is the optional bridge. Without it the
pump is the aggregate worker, which sheds a stale backlog at block
boundaries (see `capture-backlog-shedding`) and feeds the engine
the samples as they arrive. With it the pump is the split worker:
each chunk passes the resampler, the drift servo is fed after every
block, and the output-side heartbeat is watched. A transport cannot
shed and resample at once because there is no bridge state in which
both branches run.

Source: `crates/noican-coreaudio/src/pump.rs`, `Pump::new`.

The pump clears the level meters and block statistics when it is
built and again when it is dropped, which the worker thread does on
its way out. The two former loops did the same at entry and exit;
tying it to the value's lifetime leaves no path that forgets one.

Source: `crates/noican-coreaudio/src/pump.rs`, `tests::Rig`.

The tests drive the pump with the real capture and output feeds
over real rings, in one thread, with the order of capture, worker
and render callbacks chosen by the test. Time is a count of
callbacks: an own-clock capture is given a rate and a drift in ppm,
and an integer accumulator decides how many capture callbacks fall
between two render callbacks. That makes hours of drift, a dropout
or a dead output a deterministic unit test. What the simulator does
not model is scheduling: callbacks never preempt the worker and
never arrive late, so it bounds the logic, not the jitter margin,
and the hardware gate for retiring the aggregate path still stands.

Source: `crates/noican-coreaudio/src/pump.rs`, `tests::the_pump_matches_the_loops_it_replaced`.

The test module keeps a copy of the two loops the pump replaced
(`tests::FormerLoops`) and requires bit-identical rendered samples
and identical overrun, underrun and fault readings under a
pseudo-random schedule that includes stalls, floods and starved
phases, for a shared clock and for 44.1 and 48 kHz own-clock
capture. It is the evidence that the move changed no behavior
while no hardware run exists for it. Delete the copy and this test
with the first change that alters the pump's behavior on purpose;
from then on the scenario tests carry the contract.
