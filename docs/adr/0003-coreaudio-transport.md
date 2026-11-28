# 0003. CoreAudio transport architecture

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the source
location of each comment at the time of the move (commit 397e3df)
is noted above its text.

## audio-stall-heartbeat

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 500–502.

Heartbeat: the device must keep delivering callbacks while
running. Three silent seconds means it stopped (unplugged mic,
coreaudiod restart, post-sleep stall) without reporting an error.

## auhal-channel-map

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 947–959.

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

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1013–1015.

Dual mono: as many client channels as the virtual output has, so a
one-to-one channel map can feed every one of them (crate::routing).
A 1-channel virtual output yields the mono format of old.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1140–1164.

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

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1193–1199.

Reads the output channel map back for the start-time diagnostics.
Called after `AudioUnitInitialize`, the point where AUHAL reconciles
its configuration with the device: before it, a read would only echo
the array the setter just accepted. On hardware the map's effect is
otherwise invisible, so this — together with the requested map — is
the first thing to look at when the virtual microphone stays silent.
Never fails the start: an unreadable map is reported as such.

## auhal-direction-scopes

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1045–1052.

One of AUHAL's two I/O directions. Each has its own element (bus),
split into a device-facing and an app-facing scope: capture enters
the input element from the device on its input scope and reaches the
app on its output scope; render enters the output element from the
app on its input scope and reaches the device on its output scope
(Apple TN2091). The setup helpers below take a direction, so each
unit's configuration names what it switches or formats instead of
spelling out that scope and element.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1254–1257.

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

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 365–376.

Captures this cycle's `frame_count` frames and wakes the worker.

On failure the fault flag is set and the error is the status the
callback returns: `paramErr` for a cycle larger than the landing
buffer (cannot happen while `kAudioUnitProperty_MaximumFramesPerSlice`
holds; the callback never allocates to compensate), `noErr` for a
failed `AudioUnitRender` (the fault flag is the report).

# Safety

`action_flags` and `timestamp` must be the arguments AUHAL passed
to the running callback.

## capture-landing-buffer

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 336–339.

Preallocated landing buffer for `AudioUnitRender` (mono,
[`MAX_CALLBACK_FRAMES`]). On the aggregate transport the render
buffer AUHAL hands over carries one channel per virtual-output
channel, so it cannot double as the mono capture target.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1026–1028.

Bounds the unit's callback size to [`MAX_CALLBACK_FRAMES`] so the
preallocated capture landing buffer always suffices (the callback
never allocates; see [`crate::callback`]).

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

Source: `crates/noican-coreaudio/src/callback.rs` lines 110–117.

The capture side's hand-off to the inference worker: pushes captured
samples into the capture ring, counting the ones a full ring drops.

A full ring means the worker (or the capture side) fell behind by more
than the whole ring, so those samples are dropped on the floor rather
than blocking the callback. A count that moves means real audio was
silently lost — audible as clipped input reaching the models — which
the frame heartbeat alone cannot show.

Source: `crates/noican-coreaudio/src/callback.rs` lines 132–133.

Queues `samples` for the worker. Never blocks; each dropped sample
is counted with a relaxed add (real-time safe).

## capture-support-classification

Source: `macos/NoicanState/Tests/NoicanStateTests/NativeRateReducerTests.swift` lines 31–33.

A device idling exactly at 48 kHz without advertising it in
its rate list is convertible too (1/1 through the same
resampler); the live probe still prefers the aggregate path.

## capture-support-in-state

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 1–3.

How a microphone can feed the 48 kHz engine, decided from the Core
Audio snapshot at device-refresh time. Carrying this here lets the
reducer pre-flight selections without touching Core Audio.

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 5–6.

The device can run at the 48 kHz engine rate: captured through
the private Aggregate Device (the original, unchanged path).

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 27–29.

Classifies a device from the two Core Audio facts the shell
samples: whether it advertises 48 kHz support, and its current
nominal rate. Pure, so the policy stays reducer-testable.

Source: `macos/NoicanState/Sources/NoicanState/InputDevice.swift` lines 54–59.

One selectable microphone as the state machine sees it: a pure value
snapshot taken by the control plane at device-refresh time. Carrying
the capture capability here lets the reducer pre-flight selections
without touching Core Audio (the list itself follows hot-plug via
`AppEvent.devicesChanged`; a running device's live rate change is
observed separately via `AppEvent.inputSampleRateChanged`).

## coreaudio-generic-pointer-read

Source: `macos/Sources/NoicanMenuBar/VirtualOutputLevelCheck.swift` lines 61–63.

An explicit typed pointer: `&value` on a generic inout would
form a raw pointer to a possibly reference-holding `T`,
which the compiler (rightly) warns about.

## coreaudio-module-layout

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1–5.

Audited AUHAL FFI and real-time callbacks for the main transport.

The preview monitor's AUHAL lifecycle lives in the [`monitor`]
submodule; shared plumbing (unit creation, property setting, render
callback attachment, disposal) is defined here and reused there.

## coreaudio-transport-and-source-codes

Source: `crates/noican-coreaudio/src/monitor.rs` lines 103–104.

`kAudioDeviceTransportTypeVirtual`: loopback drivers (`BlackHole`,
JoyCast, ...).

Source: `crates/noican-coreaudio/src/monitor.rs` lines 106–107.

`kAudioDeviceTransportTypeAggregate`: aggregate and Multi-Output
devices composed in Audio MIDI Setup.

Source: `crates/noican-coreaudio/src/monitor.rs` lines 109–109.

`kAudioDeviceTransportTypeBuiltIn`: the Mac's own output.

Source: `crates/noican-coreaudio/src/monitor.rs` lines 111–113.

Built-in output data source for the internal speakers
(`kAudioDevicePropertyDataSource` value `'ispk'`; the headphone jack
reports `'hdpn'`).

## coreaudio-transport-architecture

Source: `crates/noican-coreaudio/src/lib.rs` lines 1–29.

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

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 344–347.

Heartbeat: input frames delivered since start (relaxed atomic add is
real-time safe). A counter that stops advancing while "running"
means the device stopped calling back (unplugged microphone,
coreaudiod restart, post-sleep stall).

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 826–828.

Total input frames the render callback has delivered since start.
A value that stops advancing while running means the device stopped
calling back; the control plane uses it as a heartbeat.

## realtime-callback-frame-cap

Source: `crates/noican-coreaudio/src/callback.rs` lines 27–29.

Largest callback an AUHAL unit may deliver, in frames. Set as
`kAudioUnitProperty_MaximumFramesPerSlice` on every unit that
captures, and the length of each preallocated capture landing buffer.

Source: `crates/noican-coreaudio/src/callback.rs` lines 32–34.

Byte size of a mono `f32` capture landing buffer holding `frames`,
or `None` when `frames` exceeds `capacity` (the buffer's length) —
the callback must then fault instead of rendering past the buffer.

## realtime-callback-rules

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1572–1581.

The interleaved `f32` samples a render callback may write — the first
buffer of the list AUHAL handed over, bounded by [`render_geometry`] —
with their channel count, or `None` for a malformed list (no buffers,
or a null data pointer). Shared by the three render callbacks
(aggregate, split output, preview monitor).

# Safety

`data` must be the non-null buffer list AUHAL passed to the running
render callback; the slice must not outlive that callback.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1600–1608.

Render callback of the aggregate AUHAL: captures the microphone frames
of this cycle into the preallocated mono landing buffer, pushes them to
the input ring, and fills the (one channel per virtual-output channel)
render buffer from the output ring, duplicating each engine sample
into every channel of its frame — the same dual-mono shape the split
transport's output callback and the preview monitor produce.
Real-time rules (docs/tech-research.md §9): no allocation, no locks;
a callback larger than the frame bound faults instead of allocating;
ring overrun drops samples; underrun renders silence.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1630–1631.

The render buffer is silenced so nothing stale can reach the
virtual output should AUHAL play it despite the error.

## render-geometry-zero-channel

Source: `crates/noican-coreaudio/src/callback.rs` lines 64–69.

Bounds a render request to the buffer AUHAL handed over.

`frame_count` frames of `number_channels` interleaved `f32` channels,
never more than `data_byte_size` bytes. A zero channel count is treated
as mono so a malformed buffer still yields silence rather than a
division by zero.

## setup-guard-ownership

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 301–303.

Owns a heap-allocated callback context during setup, reclaiming it
on any early error return (safe because the audio unit is never started
on those paths, so no callback can observe the pointer).

## split-transport-architecture

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 1–41.

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
The output ring is primed with [`OUTPUT_PRIME_SAMPLES`] of silence,
which is both the initial jitter cushion and the servo's occupancy
target; neither the servo nor the cushion depends on the ratio.

Real-time rules (docs/tech-research.md §9) hold as on the aggregate
path: the capture callback only calls `AudioUnitRender` into a
preallocated buffer, pushes into a lock-free ring, and signals the
worker's semaphore; the output callback only pops the output ring
(silence on underrun); all resampling runs on the worker with state
preallocated at start.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 68–75.

Native-rate samples drained from the capture ring per worker pass
(also the resampler's preallocation unit): 480 native samples cover
20–60 ms of telephony-profile audio, comfortably above Bluetooth
burst sizes, and ≈11 ms at 44.1 kHz (about one engine block per
pass). The resampler's per-call output bound
([`InputResampler::max_output_len`]) sizes the worker's conversion
buffers, so a 96 kHz device (half an engine block per pass) needs no
separate tuning.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 99–104.

The output ring and its underrun counter, with the same
complete-starvation definition as the aggregate path. On this
transport the ring is primed with [`OUTPUT_PRIME_SAMPLES`] of
silence, so the feed counts from the first callback on, and a
fully drained ring means the worker (or the capture side) fell
behind by more than the whole cushion.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 133–134.

Validate the rate (and preallocate the conversion state) before
touching any audio object.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 180–181.

Start the units, unwinding everything on failure (the RAII guards
release the units and contexts; the worker is stopped explicitly).

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 254–256.

Input-only AUHAL on the microphone: output disabled, capture client
format at the device's native rate (AUHAL performs no sample-rate
conversion on the input side, so the client rate must match).

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 334–336.

Registers `context` as the capture unit's input callback (the
input-only AUHAL notification slot, distinct from the render-callback
slot used by output units).

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 371–372.

Converted samples per pass, bounded by the resampler at the
largest drift correction so the buffer never grows afterwards.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 394–395.

Block until the capture callback signals more input (or
the timeout elapses, so shutdown is always noticed).

## split-transport-contract

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 750–775.

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

Source: `crates/noican-ffi/src/lib.rs` lines 223–248.

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

Source: `crates/noican-coreaudio/src/routing.rs` lines 303–318.

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

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 9–10.

Microphone the running transport was built around (the aggregate
is composed at start time, so a live change means a rebuild).

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 45–47.

Microphone to compose the aggregate around. Captured at claim
time: the *selection* can be reassigned by device hot-plug during
the busy window, but the transport is bound to this device.

## transport-pointer-ownership

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 571–574.

The AUHAL instance(s) a running transport owns, with their callback
contexts. Raw pointers are stored as integers so the runtime stays
`Send`; they are only dereferenced on the control thread that owns
the runtime, after callbacks have been stopped.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 617–622.

Disposes the audio unit(s) and reclaims the callback context(s).

# Safety

Callbacks must no longer be running (units stopped, worker
joined), and this must be called at most once.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 806–807.

Units are stopped and the worker joined, and `running` guards
against a second call.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1340–1342.

Uninitializes and disposes a stopped audio unit. Callbacks must no
longer be running (the unit was stopped, and for the main transport
the worker joined).

## transport-session-accessors

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 272–274.

The live transport in a settled state: running, or failed with the
transport still up (an engine fault). Nil while busy — the busy
transition owns its session and no new transition may start.

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 295–299.

Whether an engine transport exists that the health poll must
watch. True through busy monitor/model transitions (the transport
stays up) and through a fault (failed with a live session); false
while off, torn down, or rebuilding (`starting` claims a fresh
transport whose watch begins on success).

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 304–311.

The session whose engine transport is currently up, *including*
through busy monitor/model transitions (which keep the transport
running) and through a fault that left it up. This is what
per-device observers must key on — the input-rate listener keyed
on `liveSession` would detach for the length of every Preview
toggle or model switch and miss a profile flip in that window.
Nil while off, torn down, or rebuilding (`starting` claims a
fresh transport).

## transport-session-listener

Source: `macos/NoicanState/Tests/NoicanStateTests/NativeRateReducerTests.swift` lines 185–187.

The input-rate listener keys on this: dropping it during a
Preview toggle or model switch would blind the observer while
the transport keeps running.

## transport-shape-decision

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 235–236.

Snapshot capability, kept as the fallback vote for the
transport-shape decision below.

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 574–596.

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
