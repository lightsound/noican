# 0004. Aggregate output routing

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## aggregate-buffer-size

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.configureBufferSize`.

256 frames is a preference, not a requirement: the transport's
ring buffers absorb whatever callback size the device settles on,
so a driver clamping to its supported range is fine.

## aggregate-channel-routing

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_start`.

Starts AUHAL on an already-created private Aggregate Device.

`virtual_output_first_channel` and `virtual_output_channel_count`
locate the virtual output's channels inside the aggregate's output
channel list: the aggregate is composed as `[microphone, virtual
output]`, so the first index is the number of output channels the
microphone itself has (0 for the built-in microphone, 2 for a USB
microphone with a stereo headphone jack) and the count is the virtual
output's channel count. The transport renders one client channel per
virtual output channel (the mono engine signal duplicated into each)
and routes them there with a one-to-one AUHAL channel map; it refuses
to start when
the range does not end at the aggregate's last output channel
(`noican_coreaudio::routing`). An empty range is rejected here,
before any weight download.

Missing model weights are downloaded first (on this control thread,
without holding the control lock).

# Safety

`handle` must be a live engine handle and `model_id` must point to a
valid NUL-terminated string for the duration of this call.

Source: `crates/noican-ffi/src/lib.rs`, `tests::start_rejects_an_empty_virtual_output_range_before_any_slow_work`.

Valid layouts — the built-in microphone (no own outputs) and a
headphone-equipped microphone (two outputs ahead of the
virtual output) — pass validation; on portable builds the
transport itself then refuses (and macOS fails on the
nonexistent device 0 or its layout), but never with the
empty-range message.

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.start`.

Starts the aggregate transport on a composed private aggregate.
The composition's `virtualOutputChannels` tells the Rust side
where the virtual output's channels sit in the aggregate's output
list (after the microphone's own output channels, if any) so the
engine output is routed there rather than into a
headphone-equipped microphone's own outputs; the Rust side
re-checks the range against the device before starting.

## aggregate-composition-order

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.create`.

Composition order is load-bearing: the microphone stays first
(clock master; aggregate input channel 0 must be the
microphone, not the loopback's own input), and the output
layout handed to the transport is derived from this same list
so the two can never disagree. Channel counts are re-read
*now*, after the 48 kHz switch above: on ADAT/S-MUX interfaces
the output channel count depends on the sample rate, so the
device-refresh snapshot may describe a state the aggregate is
not composed from. The snapshot is only the fallback for an
unreadable configuration (the Rust side still checks the
result against the aggregate's own channel count).

## aggregate-device-clocking

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.create`.

Switch the subdevices themselves to 48 kHz before composing the
aggregate: the aggregate follows its clock master (the mic), and
setting the rate on the aggregate alone is rejected ('nope') by
some configurations while the mic idles at another rate (16 kHz
Bluetooth profiles, 44.1 kHz defaults, ...).

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.create`.

Only the virtual output is drift-compensated against
the microphone's clock.

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.ensure48k`.

Switches `device` to 48 kHz (polling the asynchronous change),
or throws when it cannot run there. Shared with the split
native-capture path, which has no aggregate but must still hold
the virtual output at the 48 kHz engine rate for consumers.

## aggregate-device-lifecycle

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.create`.

A thrown create must leave nothing behind: the aggregate
above is this object's only recorded state, so the caller
cannot reach it to destroy it itself.

## aggregate-device-threading

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice`.

Owns the private Aggregate Device combining the physical input and the
virtual output. `@unchecked Sendable`: creation happens on the detached
start task and teardown on the main actor, but never concurrently —
`AppState.isBusy` serializes every operation that touches this object.

## aggregate-output-channel-routing

Source: `macos/NoicanState/Sources/NoicanState/VirtualOutputChannels.swift`, `VirtualOutputChannels`.

Where the virtual output's channels sit in the private aggregate's
output channel list.

An Aggregate Device exposes its subdevices' output channels
concatenated in composition order. Noican composes
`[microphone, virtual output]` (the microphone must stay first: it is
the clock master, and the capture side relies on aggregate input
channel 0 being the microphone), so a microphone with its own output
channels — a USB microphone with a headphone jack such as the Shure
MV7+, or an audio interface — pushes the virtual output back by that
many channels. The Rust transport needs this position to route the
engine output to the virtual output instead of AUHAL's default
(device channel 0, which on such a device is the microphone's own
headphone output — recordings from the virtual microphone were silent
while the preview kept working). Mirrors `noican_engine_start` in
noican.h; the Rust side re-checks the range against the channel count
the aggregate actually reports.

Pure arithmetic, kept in this package so it is unit-tested on every
CI target; the app's `AggregateDevice` feeds it the channel counts of
the very subdevice list it composes.

Source: `macos/NoicanState/Tests/NoicanStateTests/VirtualOutputChannelsTests.swift`, `VirtualOutputChannelsTests`.

The Rust side (`noican_coreaudio::routing`) tests the channel map built
from these numbers; this suite covers the arithmetic that produces them
from the composed subdevice list — the only number in the chain that
depends on real device data.

Source: `macos/NoicanState/Tests/NoicanStateTests/VirtualOutputChannelsTests.swift`, `VirtualOutputChannelsTests.headphoneJackMicrophone`.

Shure MV7+: stereo headphone jack ahead of the virtual output.

Source: `macos/NoicanState/Tests/NoicanStateTests/VirtualOutputChannelsTests.swift`, `VirtualOutputChannelsTests.channelCountPassThrough`.

Zero is reported as-is; the Rust side refuses an empty range
with a precise message rather than this type guessing.

## aggregate-output-routing

Source: `crates/noican-coreaudio/src/routing.rs`.

Output-channel routing for the aggregate transport: where the virtual
output sits inside the private Aggregate Device, and the AUHAL channel
map that steers the engine output there.

# The bug this closes

The private aggregate is composed as `[microphone, virtual output]`
(the microphone is the main subdevice and clock master), and an
aggregate's output channels are the concatenation of its subdevices'
output channels in that order. AUHAL's default output channel map is
the identity: client channel 0 → device output channel 0, every other
device channel silent. That only reaches the virtual output when the
microphone has no output channels of its own (the built-in
microphone). A composite input/output device — a USB microphone with
a headphone jack such as the Shure MV7+, or any audio interface — puts
its own output channels first, so the processed voice went to the
microphone's headphone output and the virtual microphone recorded
silence. The preview monitor was unaffected (it has its own AUHAL on
the default output), which is why the symptom was "preview works,
recordings are silent".

# Chosen design: dual mono, one-to-one channel map (B′)

The transport renders a client stream with **as many channels as the
virtual output has**, and the render callback writes the mono engine
sample into every channel of each frame (exactly what the split
transport's output callback and the preview monitor already do). The
AUHAL channel map then only has to *place* those channels: virtual
output channel `i` receives client channel `i`, and every device
channel ahead of the virtual output — the microphone's own outputs —
is `-1` (silent). On the built-in-microphone layout the map is
`[0, 1]`, on a stereo-headphone microphone `[-1, -1, 0, 1]`.

This replaces the first fix (PR #26), which kept a mono client stream
and mapped it to the virtual output's *first* channel only
(`[-1, -1, 0, -1]`). Hardware measurement (2026-09-05, built-in
microphone, Passthrough at 100%) showed what that meant for
consumers: virtual-microphone channel 0 at −16.5 dBFS, channel 1
silent — heard in the left ear only on headphones, and 6 dB quieter
than intended in the many meeting applications that average a stereo
input down to mono — while the split transport and the monitor fed
both channels. Three signal paths with two different channel shapes
is the inconsistency this design removes.

Alternatives weighed:

- **Fan out in the channel map** (`[-1, -1, 0, 0]`: one client channel
  named by several device channels). Smallest code change, but no
  primary source (AUHAL headers, TN2091, Apple's "Channel Maps" notes
  as reproduced in `PortAudio`) states that a device channel may
  share a client channel, and a rejected or silently trimmed map
  would break every aggregate start — including the built-in
  microphone layout that has always worked. A one-to-one map, by
  contrast, is the documented shape, and its single-entry form
  (`[-1, -1, 0, -1]`) was accepted and read back unchanged on
  hardware (an MV7-class microphone, 4-channel aggregate); the
  two-entry form this module now builds is pinned by the composite
  microphone acceptance checklist rather than measured yet. Rejected.
- **Multichannel client stream, duplicate in the callback** (this
  design). The duplication is a per-frame copy the other two paths
  already perform, and the map stays one-to-one. It costs the
  render callback a separate preallocated capture landing buffer
  (the mono render buffer could no longer double as the
  `AudioUnitRender` target) — bounded by
  `kAudioUnitProperty_MaximumFramesPerSlice`, as on the split path.
  Chosen.
- **Keep the single-channel map and document the asymmetry.** Leaves
  the left-ear-only and −6 dB consumer behaviour in place. Rejected.

The design assumes nothing about the virtual output's channel count
beyond "at least one": with a 1-channel virtual output (the Noican
driver from 0.2.0 on — docs/driver.md, "History" — shaped like
Krisp's or `JoyCast`'s) the client format is mono, the map ends in a
single `0`, and the callback's per-frame loop writes one sample; with
a 2-channel one (stock `BlackHole` 2ch, the 0.1.0 driver) it is dual
mono. Nor does it depend on the virtual output having input channels;
an output-only device changes nothing here.

The virtual output's position is computed by the control plane, which
composes the subdevice list and therefore knows the layout by
construction, and is validated here against the channel count AUHAL
reports for the device: a mismatch refuses to start instead of
misrouting silently, which is exactly the failure class this module
exists to prevent.

# Rejected alternatives to the map itself

- **Reorder the subdevices to `[virtual output, microphone]`.** The
  `BlackHole`-derived loopback registers input channels too, so the
  aggregate's *input* channel 0 would become the loopback's own input
  and the (unchanged, identity-mapped) capture side would record the
  loopback instead of the microphone: the silent-microphone bug
  would only change sides. The capture invariant — aggregate input
  channel 0 is the microphone — must hold, and this fix leaves the
  input side untouched.
- **Render a device-wide multichannel client format and write the
  right slots in the callback.** Re-implements in real-time code
  what AUHAL's channel map does at setup time, and the callback
  would have to know the microphone's output channel count.
- **Discover the layout on the Rust side** (`kAudioAggregateDevice-
  PropertyActiveSubDeviceList` plus per-subdevice stream
  configurations). Self-contained, but it adds a second block of
  unsafe Core Audio property code, duplicates the virtual-output
  identification policy that lives in the Swift device catalog, and
  still needs the same device-count check to be trusted. The control
  plane already has every number involved.

# The split transport's render format

The split transport (non-48 kHz microphones, `macos::split`) has no
aggregate and no map: its output-only AUHAL sits on the virtual
output device alone, so AUHAL's identity map is already right. What
it does share with the aggregate path is the dual-mono contract — one
client channel per virtual-output channel — and until PR #29 its
render format was hard-wired to two channels, i.e. to the 0.1.0
driver. [`split_render_channels`] replaces that constant with the
channel count read from the device, so a 1-channel virtual output
(the Noican driver from 0.2.0 on, shaped like Krisp's or `JoyCast`'s)
gets a mono client format and a 2-channel one (stock `BlackHole` 2ch,
the 0.1.0 driver) keeps its stereo one; whether AUHAL would even
accept a 2-channel client format on a 1-channel device is unverified,
and matching the device removes the question.

Where the count comes from was decided against these criteria: no FFI
change (the `noican_engine_start_native` signature is a three-file
contract — `noican.h`, `RustEngine.swift`, `noican-ffi` — and the
Swift call site lives in a file at its lint length limit); as few
Core Audio behaviours as possible that hardware has not already
shown; every virtual output (1 ch, 2 ch, stock `BlackHole` 2ch)
served by the same code; a refusal, never a silent fallback, when
the count is unusable; and the real-time rules of the output
callback untouched.

- **Read it on the Rust side from the output-only AUHAL** — the
  device-side stream format of the output element, once the current
  device is set (`kAudioUnitScope_Output`, element 0; TN2091: the
  device format, never writable, so the client format cannot leak
  into the read). This is the exact read the aggregate path already
  performs on its unit (`device_output_channels`), where hardware has
  shown it to return the device's total output channel count
  (2026-09-05, 2- and 4-channel aggregates). No FFI change; the
  value describes the very device the unit is bound to; one call, and
  its only unverified aspect is that the same property behaves the
  same on an output-only unit — which hardware acceptance pins
  (`docs/macos-hardware-test.md`, native-rate section: the `Split
  output routing` line). **Chosen.**
- **Pass it from Swift through the FFI**, as the aggregate path does
  (`AudioDeviceCatalog.liveOutputChannelCount` after the 48 kHz
  switch). Rejected: it costs the three-file FFI change, and the
  number it carries is not a *composed* layout that needs checking
  against the device — on this path the virtual output *is* the
  device, so the Swift value and the AUHAL read would describe the
  same single device through two Core Audio APIs. The aggregate path's
  double check guards a derived position (microphone outputs ahead of
  the virtual output); nothing is derived here. The read-back after
  `AudioUnitInitialize` recorded in the routing description gives the
  same diagnostic value without the extra channel.
- **Both: Swift passes the count and Rust cross-checks it against the
  AUHAL read** (the aggregate pattern verbatim). Superset of the two
  above in cost — the FFI change *and* the AUHAL read — and the
  cross-check can only turn a start that would work into a refusal
  (a disagreement between two readings of one device is a Core Audio
  fault, not a layout the app can act on). Rejected.
- **Read `kAudioDevicePropertyStreamConfiguration` on the device ID
  from Rust** (the property the Swift catalog uses). Same result as
  the chosen option with a second, larger block of unsafe property
  code (a variable-size `AudioBufferList`), where the AUHAL read is a
  fixed-size struct through a helper that already exists. Rejected.
- **Set no client format and take AUHAL's default.** The default
  stream format of an audio unit is non-interleaved 32-bit float
  (Audio Unit Programming Guide, "Commonly Used Properties") — one
  buffer per channel — while the output callback writes a single
  interleaved buffer; it would silence every channel but the first,
  and AUHAL's default channel count on a freshly bound device is not
  documented. Rejected.
- **Keep a mono client format and let AUHAL fan out.** AUHAL does
  not: the identity map sends client channel 0 to device channel 0
  and leaves the rest silent — the left-ear-only result PR #26
  measured on the aggregate path. Rejected.

No candidate dominating the chosen one was found: every alternative
either adds the FFI change, adds unsafe code, or adds an unverified
AUHAL behaviour without removing the one read the chosen option
makes.

Source: `crates/noican-coreaudio/src/routing.rs`, `VirtualOutputChannels`.

Position of the virtual output's channels within an Aggregate Device's
output channel list.

`first` is the number of output channels contributed by the subdevices
ahead of the virtual output (0 for a microphone without outputs, 2 for
a microphone with a stereo headphone jack), and `count` is the virtual
output's own channel count — which is also the channel count of the
client stream the transport renders. Constructed by the control plane
from the subdevice list it composes; see [`render_channel_map`] for
how it is checked against the device.

Source: `crates/noican-coreaudio/src/routing.rs`, `VirtualOutputChannels::new`.

Describes `count` virtual-output channels starting at device
output channel `first`.

# Errors

Returns [`CoreAudioError::OutputRouting`] when `count` is zero (a
virtual output without output channels cannot receive audio) or
when the range does not fit a `u32`.

Source: `crates/noican-coreaudio/src/routing.rs`, `render_channel_map`.

Builds the AUHAL output channel map that places a `target.count()`-
channel client stream on `target` in a device with `device_channels`
output channels.

The map holds one `i32` per device channel: the client channel that
feeds it, or [`UNMAPPED_CHANNEL`]. Virtual output channel `i`
receives client channel `i` (one-to-one — the render callback puts
the same engine sample into every client channel, see the module
docs for why the duplication is not expressed in the map), and every
channel ahead of the virtual output is silent.

The map is only valid when `target` ends exactly at the device's last
channel: the control plane composes the aggregate as
`[microphone, virtual output]`, so the virtual output's channels are
the trailing ones and `target.end()` must equal `device_channels`. Any
other value means the composition and the device disagree about the
layout, and the transport must not guess where the virtual output is.

# Errors

Returns [`CoreAudioError::OutputRouting`] when `target.end()` differs
from `device_channels`.

Source: `crates/noican-coreaudio/src/routing.rs`, `render_channel_map`.

In range by construction (`count >= 1` and `end() == device_channels`);
plain indexing rather than a silent `get_mut`, because an all-`-1`
map would be exactly the silent virtual microphone this module
prevents.

## aggregate-rate-polling

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.configureSampleRate`.

Nominal-rate changes propagate to the subdevices asynchronously,
and some drivers reject the set while already at the target rate,
so poll the read-back instead of trusting one set + one read
(a single immediate read loses the race and made every start fail
while the microphone idled at 44.1 kHz). Runs on the background
start task, never on the main actor.

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.configureSampleRate`.

Re-issue once midway in case the first set landed before the
subdevices had finished attaching.

## aggregate-transport-contract

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::start`.

Opens an initialized AUHAL on a private Aggregate Device.

`aggregate_device` must contain the selected physical input and the
`BlackHole` output subdevice with drift compensation configured by the
Swift control plane. `virtual_output` says where that subdevice's
channels sit in the aggregate's output channel list (after any
output channels of the microphone itself); the engine output is
rendered as one client channel per virtual-output channel (dual
mono) and routed there with an explicit one-to-one AUHAL channel
map, see [`crate::routing`].

`levels` receives per-block input/output peak meters from the
inference worker for the lifetime of this runtime; the worker
resets it to silence on start and on exit. `monitor_state` is the
shared [`crate::monitor::MonitorState`] cell: monitor toggles move
it between off and playing here, the worker's feedback guard moves
it to tripped, and keeping it caller-owned lets the control plane
poll it without any lock. It reads off whenever this runtime is
down (reset on start and by the stop path's disable).

# Errors

Returns [`CoreAudioError`] when AUHAL setup or worker startup fails,
including [`CoreAudioError::OutputRouting`] when `virtual_output`
does not match the output channel count the aggregate reports.
Every error path releases the AUHAL instance, the callback context,
and the worker (RAII guards; nothing leaks on failed starts).

## audio-stall-detection

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.audioStalled`.

The health poll saw no frames for three consecutive seconds while
running (unplugged mic, coreaudiod restart, post-sleep stall).

## dual-mono-rendering

Source: `crates/noican-coreaudio/src/callback.rs`, `fill_dual_mono`.

Writes one mono engine sample per frame into an interleaved render
buffer, duplicated into all of the frame's `channels` (dual mono).

This is the shape all three render callbacks (aggregate, split
output, preview monitor) produce. `next` supplies the samples, one
call per frame; a frame it cannot serve (`None`) renders silence. A
zero channel count is treated as mono, as in [`render_geometry`].
Returns whether any frame received a real sample.

## loopback-device-exclusion

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift`, `AudioDeviceCatalog.isNoicanVirtualDevice`.

True for a loopback device Noican can route into: stock BlackHole
2ch (the Phase 0 device) or the Noican-branded driver. It registers
input channels too, but selecting it as the microphone would only
feed the loopback back into itself, so pickers must exclude it.

Keep in sync with the Rust preview-monitor policy
(`classify_monitor_target` in crates/noican-coreaudio/src/monitor.rs),
which matches the same UIDs and additionally rejects any
virtual/aggregate transport.

Source: `macos/Sources/NoicanMenuBar/CoreAudioDevices.swift`, `AudioDeviceCatalog.isSelectableInput`.

True for devices that make sense as the physical microphone. Virtual
devices (JoyCast, other loopbacks — they report the `virt` transport
type) and aggregates are excluded: routing another virtual mic into
Noican would double-process or loop audio, and fixed-rate virtual
subdevices also make the private aggregate reject the 48 kHz setup.

## split-output-routing

Source: `crates/noican-coreaudio/src/macos/split.rs`, `build_output_half`.

Builds the output half of the split transport: an output-only AUHAL
on the virtual output with the render callback attached and the unit
initialized, ready to be started. Also returns the routing
description for [`Runtime::routing_description`]: the virtual
output's reported channel count, the render format set from it, and
that format read back after `AudioUnitInitialize`.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `describe_applied_render_format`.

Reads the client render format back for the start-time diagnostics,
after `AudioUnitInitialize` — the point where AUHAL reconciles the
client format with the device (the aggregate path reads its channel
map back at the same point, for the same reason). Never fails the
start: an unreadable format is reported as such.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `configure_output_auhal`.

Output-only AUHAL on the virtual output: input disabled, mono 48 kHz
engine samples rendered as interleaved packed float with **one
client channel per virtual-output channel**. Returns that channel
count. The control plane switches the virtual output device itself
to 48 kHz before starting this transport (it feeds consumers and
must stay at the engine rate), so no device-side rate conversion is
involved.

The channel count is read from the device-side format of the output
element once the current device is set — the same read the aggregate
path performs for its channel map ([`device_output_channels`]), here
on an output-only unit — and validated by
[`split_render_channels`] (a device reporting no output channels
refuses the start; there is no fallback width). The decision record
for reading it here rather than receiving it from the control plane
is in [`crate::routing`] ("The split transport's render format").
Whether the read behaves on an output-only unit exactly as on the
aggregate unit is what the hardware `Split output routing` log line
pins (docs/macos-hardware-test.md).

No channel map is needed here, unlike the aggregate path
([`crate::routing`]): this unit sits on the virtual output device
alone, so AUHAL's identity map already lands client channel `i` on
the virtual output's channel `i` for every channel the device has —
a microphone's own output channels never appear in front of them on
this transport. The render callback sizes its per-frame duplication
from the buffer AUHAL hands it, so the callback code is the same for
any width.

## split-output-stall-detection

Source: `crates/noican-coreaudio/src/macos/split.rs`, `OUTPUT_STALL_BLOCKS`.

Consecutive engine blocks the output callback may stay silent before
the worker flags a fault (3 s of processed capture at 10 ms blocks,
matching the control plane's stall horizon). On the aggregate path a
dead output device kills the single shared callback and trips the
frame heartbeat; on the split path the capture callback keeps that
heartbeat advancing, so the output side carries its own pulse
counter and the worker faults when it stops moving while capture is
alive. Watching the callback pulse — not ring occupancy — keeps a
transient ring-filling hiccup from latching a false fault: a full
ring drains only at the servo's ppm trickle, but a live callback
keeps pulsing regardless.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `OutputContext::pulses`.

Heartbeat: render callbacks delivered since start (relaxed
atomic add is real-time safe). The worker watches it to detect a
virtual-output side that stopped calling back (see
[`OUTPUT_STALL_BLOCKS`]).

Source: `crates/noican-coreaudio/src/macos/split.rs`, `split_processing_loop`.

Output-side heartbeat (see OUTPUT_STALL_BLOCKS): blocks
only run while capture delivers, so a pulse counter that
stops moving here means the output callback went silent
while the microphone is still alive.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `output_render_callback`.

Heartbeat for the worker's output-side stall watch (relaxed
atomic add: real-time safe, like the capture frame counter).

## transport-routing-by-device

Source: `macos/Sources/NoicanMenuBar/AggregateDevice.swift`, `AggregateDevice.ensure48k`.

Defensive only: microphones that cannot run at 48 kHz
(Bluetooth telephony profiles) are routed to the split
native-capture transport by the control plane (issue #7)
and never reach the aggregate path.

## transport-routing-diagnostics

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_routing_description`.

Diagnostic: how the running transport routes the engine output into
the virtual output.

On the aggregate transport the description carries the output channel
count AUHAL reports for the aggregate, the channel map requested, and
the map read back after `AudioUnitInitialize`; on the split transport
it carries the output channel count AUHAL reports for the virtual
output device, the render format sized from it, and that format read
back after initialize. Copies the description as UTF-8 and returns
the required byte count including the terminating NUL; returns 0
while stopped or for a null handle. Takes the control mutex (meant
for the one-time start log, not the poll path).

# Safety

`handle` must be null or a live engine handle. A non-null `buffer`
must be writable for `capacity` bytes.

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.routingDescription`.

Diagnostic: how the running transport routes the engine output
into the virtual output — on the aggregate transport the
aggregate's reported output channel count, the channel map
requested, and the map read back after `AudioUnitInitialize`; on
the split transport the virtual output's reported channel count,
the render format sized from it, and its read-back — or nil while
stopped. Reads the control mutex — for the one-time start log
only.

## virtual-output-routing-fail-closed

Source: `crates/noican-coreaudio/src/lib.rs`, `CoreAudioError::OutputRouting`.

The engine output could not be routed to the virtual output's
channels: inside the Aggregate Device the layout the control plane
composed does not match the channel count the device reports, or
the described range is empty; on the split transport the virtual
output device reports no output channels to size the render
format from (see [`routing`]). Refusing to start is deliberate — a
guessed map or width could feed a microphone's headphone output,
or be refused by AUHAL, and leave the virtual microphone silent.
