# macOS Build and Hardware Test Plan

## Verification boundary

The Linux CI covers the common engine, real ONNX inference, multi-format CLI
input (WAV/AIFF/AIFC/CAF/M4A), lock-free model switching, the native-rate
input resampler and clock-drift servo (per-ratio delay reporting, drift
cancellation, strength alignment through the resampler), and the C ABI. The
macOS CI job additionally covers clippy/tests for `aarch64-apple-darwin`
(including the AUHAL transport), SwiftLint in strict mode, the release
app build with `swift -warnings-as-errors`, and an ad-hoc build of the
Noican driver (compile check only — coreaudiod will not load an ad-hoc
signature; see docs/driver.md).

Physical microphone capture, TCC prompts, aggregate clock behavior,
BlackHole routing, audible switching, 16 kHz-model audio quality, and
long-session stability require Apple hardware. So do the Phase 2
controls: login-item registration (`SMAppService` depends on the app's
location and signature — CI only compiles that path), the audible
quality of the dry/wet strength mix, and preference restoration across
real relaunches. **They are not claimed as verified until every
applicable check below is recorded.** The transport design passed this
plan on macOS 26 / Apple Silicon in its candidate-B incarnation; the
hybrid build (C engine + B transport) must be re-accepted.

## Prerequisites

- Apple Silicon Mac running macOS 14.2 or newer.
- Xcode command-line tools.
- Rust 1.98.0 with the `aarch64-apple-darwin` target.
- Swift 6.1 or newer and SwiftLint.
- A Developer ID Application identity for a distributable app build.
- A loopback driver: the Noican driver built from this repository
  (`scripts/build-driver.sh`, Developer-ID-signed; see docs/driver.md), or
  stock BlackHole 2ch as the Phase 0 fallback.
- Headphones. Phase 0 has no AEC and must not be evaluated through speakers.
- A 48 kHz-capable microphone (the built-in microphone works) for the
  aggregate-path checks, and a microphone that cannot run at 48 kHz for
  the native-capture checks — any device from 8 to 192 kHz is captured
  natively through the split transport and resampled to 48 kHz inside
  it by the exact ratio (issue #7): a Bluetooth headset on a telephony
  profile (HFP/SCO at 8/16/24 kHz), or a 44.1 kHz-family device
  (44.1/22.05/11.025 kHz, e.g. a headset whose microphone only exposes
  CD-family rates). For telephony profiles expect narrow-band quality,
  and expect the headset's *playback* quality to drop while its
  microphone is in use (the whole headset falls into the phone profile)
  — both are properties of Bluetooth, noted in the UI, not defects. A
  44.1 kHz device is full-band; only the conversion is noted. Devices
  whose rate is unreadable or outside 8–192 kHz are refused.
- A **composite input/output microphone** for the aggregate-routing
  checks: a 48 kHz-capable USB microphone that also exposes output
  channels (a headphone jack — e.g. the Shure MV7+), or an audio
  interface with both inputs and outputs. Such a device appears in Audio
  MIDI Setup as *one* device with both an input and an output side; a
  headset that shows up as two separate devices (input-only plus
  output-only, as some Bluetooth headsets do) does not exercise this
  path.

The BlackHole-derived driver is GPL-3.0 and remains a separate program. Do
not add its source or object files to the application target.

## Build

```bash
rustup toolchain install 1.98.0 \
  --target aarch64-apple-darwin \
  --component clippy,rustfmt

cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked \
  --target aarch64-apple-darwin

swiftlint lint --strict --config .swiftlint.yml

# Ad-hoc local build (swift build runs with -warnings-as-errors):
bash scripts/build-macos-app.sh

# Developer ID build:
NOICAN_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
  bash scripts/build-macos-app.sh
```

Expected artifact: `dist/Noican.app`.

The build replaces the bundle on disk but does not touch a running
instance: after every rebuild quit the app (`pkill -x NoicanMenuBar`),
`open dist/Noican.app`, and confirm the PID in the Console lines has
changed before testing — a 2026-09-05 run reported a fix as ineffective
because the pre-fix process was still the one under test. If `swift
build` fails to find a type that exists in `macos/NoicanState`, remove
the stale SwiftPM caches (`rm -rf macos/.build macos/NoicanState/.build`)
and rebuild.

Validate it:

```bash
codesign --verify --deep --strict --verbose=2 dist/Noican.app
codesign --display --verbose=4 dist/Noican.app
```

The app is a UI agent (`LSUIElement`) and therefore has no Dock icon.

## Model weights

The app downloads missing weights on demand (control thread, never the
audio path) into `~/Library/Application Support/noican/models`
(override: `NOICAN_MODELS_DIR`). To avoid mid-test downloads, pre-fetch
into the same directory:

```bash
NOICAN_MODELS_DIR="$HOME/Library/Application Support/noican/models" \
  cargo run -p noican-cli --release -- \
  --models-dir "$HOME/Library/Application Support/noican/models" fetch
```

## Driver check

The subject is the Noican driver (docs/driver.md). Stock BlackHole 2ch
remains acceptable as the Phase 0 fallback, but the acceptance record for
Phase 1 must cover the Noican driver.

1. Build the driver with a Developer ID identity and record the exact
   command and output:

   ```bash
   NOICAN_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
     bash scripts/build-driver.sh
   ```

2. Install it (copies to `/Library/Audio/Plug-Ins/HAL/Noican.driver` and
   restarts the audio daemon):

   ```bash
   bash scripts/install-driver.sh
   ```

   If restarting manually: on macOS 26, SIP rejects
   `sudo launchctl kickstart -k system/com.apple.audio.coreaudiod`; use
   `sudo killall coreaudiod` instead.
3. Open Audio MIDI Setup and confirm **Noican Microphone** appears as a
   two-channel 48 kHz device (manufacturer `lightsound`).
4. Confirm the device has both output and input streams and can loop a test
   signal before involving Noican.
5. Record the installed bundle signature:

   ```bash
   codesign --verify --deep --strict --verbose=2 \
     "/Library/Audio/Plug-Ins/HAL/Noican.driver"
   ```

6. Coexistence: with stock BlackHole 2ch also installed, start the engine
   and confirm the private aggregate is composed around the **Noican**
   device (`com.lightsound.noican.2ch_UID`), not `BlackHole2ch_UID` —
   the app prefers the Noican driver when both are present
   (docs/driver.md, "Coexistence"). Then uninstall stock BlackHole (or
   test the Noican-only state first) and confirm the app still selects
   the Noican device.
7. Uninstall check (after the functional tests): `bash
   scripts/uninstall-driver.sh`, then confirm no Noican device remains in
   Audio MIDI Setup and `/Library/Audio/Plug-Ins/HAL/Noican.driver` is
   gone.

Do not disable SIP or use an ad-hoc driver signature for the acceptance test.

## Functional test

1. Connect wired headphones.
2. Launch `dist/Noican.app`.
3. Confirm the menu bar popover shows:
   - the status header with a state indicator,
   - the Off / Preview / On mode control (sliding-pill segments),
   - the Microphone list showing every physical input with a checkmark
     on the selection,
   - a "Model & strength" disclosure row (below the Microphone list),
     collapsed on first launch; its expansion state is remembered
     across launches, and while collapsed the row shows the active
     model and strength (e.g. "FastEnhancer-B 48k · 100%"). Expanding
     it reveals the Model selector and the Strength slider.
   - the Model selector (inside "Model & strength"): **every registry
     stage** as rows with a checkmark on the selection — Passthrough, FastEnhancer
     T/B/S/M/L, DPDFNet2, DPDFNet8, DeepFilterNet3, UL-UNAS, Hush, and
     TSE Conv-TasNet 48k disabled as "requires enrollment"; the default
     model row is annotated "Default". Hovering a row pops the model's
     profile card out beside that row after a short delay: name, tag,
     four dot ratings (Noise removal / Voice quality / Responsiveness /
     Efficiency, all "more is better"), and the raw facts (native rate,
     measured delay, size). Once up, the card must **stay up while the
     pointer moves between rows, following the hovered row's position
     and swapping its content in place** (no per-row blink or
     re-present animation), hide shortly after the pointer leaves the
     rows, and — critically — hovering must never close the menu
     popover itself.
   The monitoring section (level bars) must be absent while the mode is
   Off and appear while the engine runs.
4. Select a physical microphone and `FastEnhancer-B 48k`.
5. Select On and grant microphone access when macOS prompts.
6. Confirm status changes to `Running` (the Model picker shows the
   active model).
7. In QuickTime, OBS, or a meeting app, select the BlackHole/Noican virtual
   device as the microphone.
8. Record at least 30 seconds containing speech, steady fan noise, and
   keyboard noise.
9. Confirm the recording is non-silent, intelligible, and materially differs
   from a raw-microphone control. Speech must survive keystrokes and claps
   (the candidate-B engine's transient over-suppression is a known failure
   mode this hybrid must not reproduce).
10. Switch to `Hush 16k` and then `UL-UNAS 16k` while recording: **both must
    produce clearly audible, intelligible speech** (the candidate-B engine's
    16 kHz path was near-silent; the hybrid routes these models through the
    verified polyphase resampler).
10b. **Hush loudness parity**: with the strength slider pinned at
    **100%** (the unity dry path is identical on both models, so any
    partial strength narrows the gap and can mask a persisting 100%
    deficit), speak the same sentence through `Hush 16k` and
    `FastEnhancer-B 48k` back to back while recording. The perceived
    speech loudness must match between the two (within about
    1 dB of voiced level in the waveform). Hush's network attenuates
    speech itself — a measured −3.4 dB to −1.5 dB voiced-frame RMS
    deficit depending on material — and the stage now compensates with
    a measured +2.45 dB makeup gain (constant and measurement recorded
    in `crates/noican-models/src/stages/dfn_tract.rs`). A clearly
    quieter Hush is the pre-fix defect and fails this check; also
    confirm no clipping or distortion on loud speech (the gain is
    applied without a limiter, a documented design decision — the
    measured post-gain peak keeps ≈1.9 dB of headroom at a 0.7-peak
    input).
11. Selecting `TSE Conv-TasNet 48k` must fail gracefully: a clear
    "requires enrollment" message under the Model picker, the engine
    still running the previous model (status stays `Running`, meters keep
    moving, pill stays green), picker reverted.
12. Select Off. Confirm the private Aggregate Device disappears and
    the virtual microphone no longer receives new processed audio.

On a failure the header status shows a one-line `Error` and the full
message renders under the mode control (the header never grows, so the
control cannot shift). Collect the macOS version, selected device UIDs,
buffer size, and Console entries. Do not characterize the path as
working.

## Model switching

While recording one continuous file:

1. Switch among `fastenhancer-t/b/s/m/l`, `dpdfnet2`, `dpdfnet8`, `dfn3`,
   `ul-unas`, and `hush`.
2. Place markers immediately before each change.
3. Inspect the waveform around each marker at sample level.
4. Pass criteria:
   - no full-scale impulse,
   - no NaN/sustained digital noise,
   - only the bounded fade-to-silence/fade-in interval (2 × 240 samples at
     48 kHz = 10 ms),
   - status reflects the selected model.

Model construction (and any weight download) occurs on the control thread.
The inference thread receives the fully prepared stage through a
preallocated lock-free queue. The Core Audio callback never loads a model.

TSE requires a valid ECAPA enrollment and authenticated, checksum-confirmed
model files as described in [models.md](models.md). It is excluded from this
step until the upstream access/license blocker is resolved and the app
grows an enrollment flow.

## Microphone switching

The private aggregate is composed around the microphone at start time,
so changing it while running rebuilds the transport.

1. While On (or Preview), select a different physical microphone in the
   list: after a brief busy state the engine must return to Running (or
   Previewing) with the same model, now capturing from the new device.
   A short gap is inherent; a crash, a stale aggregate in Audio MIDI
   Setup, or a dead stream is a failure.
2. Newly connected input devices must appear in the list within a
   moment, without reopening anything; disconnected ones must disappear.
3. Select a non-48 kHz microphone (a Bluetooth headset, or a 44.1 kHz
   device) while Off, then select On: the engine must start through the
   split native-capture transport and reach Running (see the dedicated
   native-rate section below) — the Phase 0 refusal of telephony-rate
   devices and the later refusal of 44.1 kHz-family devices are both
   gone.
4. *(Requires a device whose rate lies outside the 8–192 kHz range the
   capture resampler converts, or whose rate is unreadable; real
   hardware like this is rare — skip when none is available. A
   44.1 kHz-only interface is **not** such a device any more: it takes
   the split path, see the next section.)* Select such a device while
   Off, then select On: the refusal must be **instant** (a pre-flight
   reads the snapshot's capability — no busy spinner, no teardown),
   with a clear reason under the mode control and the pill **staying on
   On** with a red warning tint (the control shows the user's intent;
   the system never moves it). Then select the built-in microphone: the
   engine must restart automatically into the selected mode and reach
   Running. Re-tapping the red segment must also retry. While running,
   clicking the unsupported device in the list must be refused in place
   — the checkmark returns to the working microphone, the reason
   appears under the list, and the engine keeps running uninterrupted.
5. If a live switch fails at runtime (a failure the pre-flight cannot
   see, e.g. the new device vanishing mid-switch), the app must fall
   back to the previous microphone automatically — one rebuild attempt,
   reason under the list — instead of leaving the session dead.

## Non-48 kHz microphone (native-rate capture)

Microphones that cannot run at 48 kHz — telephony-profile Bluetooth
headsets (HFP/SCO at 8/16/24 kHz) and 44.1 kHz-family devices
(44.1/22.05/11.025 kHz; 88.2/96 kHz-only interfaces too) — are captured
through a **split transport** instead of the private Aggregate Device:
an input-only AUHAL on the microphone at its native rate, an output-only
AUHAL on the virtual output at 48 kHz, and the inference worker bridging
the two clock domains through one arbitrary-ratio polyphase FIR
resampler (`PolyphaseResampler` in `noican-core`): the exact reduced
ratio — 160/147 for 44.1 kHz, 3/1 for 16 kHz — with the clock-drift
correction folded into its fractional phase step and steered by ring
occupancy (there is no aggregate to absorb the clock split, so drift
compensation is our own — docs/tech-research.md §4.2). The split path
adds a ~50 ms output cushion; the 48 kHz aggregate path is untouched.
The engine still runs at 48 kHz throughout, so recordings from the
virtual device remain 48 kHz regardless of the capture rate.

Before testing, confirm the device's actual native rate: open Audio
MIDI Setup, select the device's input side, and read the Format
pop-up's rate list — or run
`system_profiler SPAudioDataType | grep -A 12 "<device name>"` and read
`Current SampleRate`. Record the rate in the result record; the
microphone list must show the same value.

1. Connect the device (pair the Bluetooth headset). Confirm the
   microphone list shows its native rate next to the name in audio
   notation (e.g. "16 kHz", "24 kHz", "44.1 kHz").
2. Select it. A secondary-style notice must appear under the list. For
   a telephony profile: capture is narrow-band (phone profile) and
   headset playback quality drops while the microphone is in use. For a
   44.1 kHz-family device: the device is resampled to the 48 kHz engine
   rate inside Noican — the notice must **not** call the audio
   narrow-band. Either way this is information, not an error — no red
   text, no refusal.
3. Select On and grant microphone access if prompted: the engine must
   reach `Running` (green indicator). No Aggregate Device appears in
   Audio MIDI Setup for this path (two AUHAL instances instead). The
   one-time transport diagnostics line in Console must show
   `worker realtime scheduling true`, and it is followed by the split
   transport's routing line — `Split output routing: virtual output
   channels N, render format requested N ch, render format read back
   after initialize N ch` — where N is the virtual output's output
   channel count as AUHAL reports it for the output-only unit (2 for
   the current Noican driver and for stock BlackHole 2ch; 1 for a
   1-channel driver). The render format is sized from that count
   rather than fixed at two channels, so all three numbers must agree
   and match the device's channel count in Audio MIDI Setup; record the
   line verbatim. A start refusal reading `virtual output routing
   failed: the virtual output device reports no output channels (split
   transport)` means AUHAL reported zero channels for the device — the
   transport refuses rather than guessing a width; record the device's
   Audio MIDI Setup channel counts.
4. Record 30+ seconds from the virtual device in QuickTime: the
   recording must be 48 kHz, non-silent, and intelligible. On a
   telephony profile expect telephony bandwidth (the source is 8–16 kHz
   capture) and expect the headset's own playback to sound worse while
   recording — both by design. On a 44.1 kHz device expect full-band
   quality indistinguishable from the aggregate path (passband flat to
   ≈15 kHz, gentle roll-off above 17 kHz): no pitch shift, no
   periodic click, no metallic/aliased coloration.
5. Speak continuously at 50% strength: the voice must stay single (no
   doubled/hollow voice) — the dry path taps the engine input *after*
   the input resampler, so the alignment must hold exactly as on the
   aggregate path.
6. Switch models while recording: same bounded-fade criteria as the
   aggregate path.
7. Switch between the native-rate microphone and the built-in
   microphone while running: each direction must rebuild into Running
   (aggregate ↔ split transport switch behind the same busy machine).
8. **Drift/endurance**: record 30 continuous minutes through the
   native-rate microphone. Pass criteria: no periodic click, no
   accumulating gap/overlap, no engine fault, no stall — the drift
   servo must keep the two clock domains aligned for the whole session.
   On a 44.1 kHz device the resampler runs at a non-integer ratio, so
   this is also the check that the servo's occupancy control holds
   there; a slow periodic pitch wobble or a click every few seconds
   would point at it.
9. **Underrun diagnostics on the split path**: while recording, watch
   Console for the underrun line (see "Output-underrun diagnostics").
   With a light model (FastEnhancer-B) the split transport must log
   zero underruns over 60+ s of continuous speech; record any line
   verbatim.
10. **Profile flip (A2DP ↔ HFP)**: while running, force a rate
    renegotiation (e.g. play music to the headset before/while starting,
    or toggle the headset's own transparency/ANC features if they
    trigger one). The app must either keep running unaffected or rebuild
    automatically within a moment (a brief busy spinner, then Running) —
    never a permanently dead session. Unplugging/re-pairing mid-session
    may still surface as "Audio stalled"/"Microphone disconnected" like
    any device loss; selecting the device again must recover.
11. **Virtual-output loss on the split path**: while running on the
    native-rate microphone, uninstall (or otherwise remove) the virtual
    output device — the engine must stop with "Virtual output device
    removed" like on the aggregate path (device-list listener). The
    split transport additionally watches for a *wedged* output side —
    the device still listed but its IO no longer calling back — via an
    output-callback pulse counter: capture alone keeps the frame
    heartbeat advancing on this path, so a pulse counter frozen for
    ~3 s of live capture raises an engine fault ("Audio fault — turn
    noise cancellation off and on") instead of rendering perpetual
    silence under a green pill. A transient hiccup that merely fills
    the output ring must *not* trip it (the callback keeps pulsing).

## Composite input/output microphone (headphone-equipped USB microphone)

The private aggregate is composed as `[microphone, virtual output]`
(the microphone is the clock master and must stay first so that
aggregate input channel 0 is the microphone, not the loopback's own
input). An aggregate's output channels are its subdevices' output
channels concatenated in that order, so a microphone that has output
channels of its own — a USB microphone with a headphone jack such as
the Shure MV7+, or an audio interface — places them *ahead* of the
virtual output. The transport rendered a mono stream, and AUHAL's
default output channel map sends client channel 0 to device output
channel 0: on such a device that is the microphone's own headphone
output, and the virtual output received silence. Recordings from the
virtual microphone were completely silent while the preview (its own
AUHAL on the default output) kept working, with no error and no
underrun line in the log; the built-in microphone, having no outputs,
was never affected — which is why earlier acceptance runs, all made
with the built-in microphone, did not see it.

The fix sets an explicit AUHAL channel map on the aggregate unit. The
transport renders **one client channel per virtual-output channel** and
writes the mono engine sample into every channel of each frame (dual
mono — the shape the split transport and the preview monitor always
produced), and the map places those client channels on the virtual
output one-to-one: virtual output channel *i* receives client channel
*i*, and every device output channel ahead of it — the microphone's own
outputs — is left silent. The virtual output's position is computed by
the control plane from the subdevice list it composes and re-checked on
the Rust side against the channel count the aggregate reports. On the
built-in-microphone layout the map is `[0, 1]`; with a stereo-headphone
microphone it is `[-1, -1, 0, 1]`.

The first version of this fix (PR #26) kept a mono client stream and
mapped it to the virtual output's first channel only, leaving channel 1
silent (measured on the built-in microphone: channel 0 −16.5 dBFS,
channel 1 silent). That was heard in the left ear only on headphones,
and consumers that average a stereo input to mono — common in meeting
applications — received it 6 dB down. Duplicating in the *map* instead
(`[-1, -1, 0, 0]`) was rejected because no primary source states that
an AUHAL map may name one client channel twice, and a rejected map
would fail every aggregate start; a one-to-one map is the documented
shape, and its single-entry form was accepted and read back on hardware
(PR #26's record) — the two-entry form is what acceptance criterion 2
below pins (see `noican_coreaudio::routing` for the full decision
record). Step 8 and acceptance criterion 3 pin the new shape:
both channels carry the same signal, and channel 0's level is unchanged
against the previous build. Note for consumers that *sum* L+R without
scaling (rare; most average): the dual-mono signal reads +6 dB there
compared with the single-channel build. The capture direction is
untouched. The split (native-rate) transport is not involved: its
output AUHAL sits on the virtual output device alone, so a microphone's
outputs never precede the virtual output there.

1. Connect the composite device and make sure it advertises 48 kHz
   (Audio MIDI Setup, input side), so the engine takes the aggregate
   path. Note its output channel count from the output side (2 for a
   stereo headphone jack).
2. Plug wired headphones into the *device's own* headphone jack, and
   make a different device (the built-in output, or other headphones)
   the system default output.
3. Select the composite device as the microphone, select `Passthrough`
   or `FastEnhancer-B 48k`, then select On. The engine must reach
   `Running`. The private aggregate is hidden from Audio MIDI Setup, so
   read its composition from Console instead (subsystem
   `com.lightsound.noican`, category `engine-diagnostics`, or the
   `log stream` command from the underrun section) — two info lines
   appear on every aggregate-path start:
   - `Aggregate composed: microphone "<name>" (in X / out Y), virtual
     output "<name>" (out Z); aggregate reports W output channel(s) in
     S stream(s)` — Core Audio's view, written when the aggregate is
     created (Y is the microphone's own output channel count, W must be
     Y + Z, and S the number of subdevices contributing outputs);
   - `Aggregate output routing: aggregate output channels N, virtual
     output at channels A..B, channel map requested [...], channel map
     read back after initialize [...]` — AUHAL's view, written about a
     second after start. N must equal W, `A..B` must be `Y..Y+Z`, the
     requested map must be `-1` at every index below A and `0, 1, …`
     (client channel *i* at index A + *i*) from A to the end — e.g.
     `[-1, -1, 0, 1]` for a stereo-headphone microphone, `[0, 1]` for
     the built-in microphone — and the read-back must equal the request.
   Record both lines verbatim in the result record.
4. Record 30+ seconds of speech from the Noican virtual microphone in
   QuickTime (or CleanShot / OBS). The recording must be non-silent,
   intelligible, and — at 100% strength — processed (materially differs
   from a raw-microphone control). This is the check that failed before
   the fix (a completely silent file).
5. While recording, listen on the headphones plugged into the
   microphone's own jack: **nothing from Noican** may come out of them
   (the device's own hardware monitoring, if it has any, is unrelated
   and may be audible — distinguish it by muting the device's monitor
   control). Processed voice on the microphone's headphone jack is the
   pre-fix misrouting and fails this check.
6. Select Preview: the processed voice must play on the system default
   output as before, the recording must continue unaffected, and the
   microphone's own headphone jack must still stay silent.
7. Switch models while recording (aggregate-path criteria: bounded fade
   only) and watch Console for the underrun line — a light model must
   log zero underruns as before.
8. Switch to the built-in microphone while running: after the brief
   busy state, recordings from the virtual microphone must still carry
   audio (regression check for the no-own-outputs layout, where the
   virtual output is at channels 0–1). Pin the signal shape, not just
   presence: record a fixed reference (a sentence at constant distance,
   or a tone played into the room) once on this build and once on the
   previous build (`main` before this change) with the same settings —
   Passthrough, strength 100%, the microphone's system input slider
   and the Noican Microphone slider both at maximum — and compare per
   channel with the script in "Level integrity" below. Expected on this
   build: **every virtual-output channel carries the same signal**
   (channel RMS within 0.1 dB of each other), and channel 0's RMS is
   within ±1 dB of the previous build's channel 0 (reference from the
   2026-09-05 measurement: −16.5 dBFS on the built-in microphone; your
   absolute figure depends on voice and distance, the *difference*
   between builds is what is pinned). A level change on channel 0
   beyond that, or a channel left silent, fails. Switch back to the
   composite device: audio must return, on both channels.
9. *(If available)* Repeat 3–4 with an audio interface that has more
   than two outputs: the virtual output sits after all of them, and the
   recording must still carry audio.

A start refusal reading `virtual output routing failed: the aggregate
device reports N output channel(s), but the virtual output was expected
at channels A..B` means the composed layout and the device disagree;
record N, A, B, the device's Audio MIDI Setup input/output channel
counts, and the `Aggregate composed` line (the only one of the two
Console lines that prints on a refusal — there is no running transport,
so the `Aggregate output routing` line never appears; N, A and B are in
the refusal message itself). This refusal is deliberate
(the alternative is a guessed map that may misroute silently); do not
characterize the path as working. The counts are re-read after the
48 kHz switch and immediately before the aggregate is composed, so a
rate-dependent channel count (ADAT/S-MUX interfaces expose 8 channels
at 48 kHz but 4 at 96 kHz) is not a cause of this refusal; if it still
appears, the first number to question is N — whether AUHAL reports the
aggregate's total output channels or only its first stream, which no
primary source states outright.

## Level integrity

What decides how loud consumers hear the virtual microphone, and how to
tell the pieces apart when "Noican sounds quiet". Established by the
2026-09-05 hardware investigation (built-in microphone, MV7i, Bluetooth
headset; all figures from QuickTime recordings measured with the script
below).

**What sets the level.**

- **The engine path is unity gain.** Passthrough at 100% strength
  changes nothing; the models change the signal, not its nominal level
  (Hush carries a measured makeup gain, see functional test 10b).
- **Noican captures the microphone at unity and does not apply the
  microphone's system input slider on the aggregate path.** Measured
  with the built-in microphone: moving System Settings › Sound ›
  Input's slider for the *microphone* from the middle to maximum
  changed a direct recording by +7.6 dB and the Noican recording by
  1.4 dB — within the spread of repeated speech, i.e. no effect. The
  aggregate's AUHAL reads the device's raw input stream. On the split
  transport (non-48 kHz microphones, which are opened directly rather
  than through an aggregate) the same holds within the evidence so far:
  the 2026-09-05 Bluetooth measurement
  ([record](acceptance/2026-09-05-dual-mono-level-integrity.md)) read
  −23.2 dBFS with the headset's slider at the middle and −21.0 dBFS at
  maximum (+2.2 dB), which the operator attributed to speech variance —
  far from the +7.6 dB a direct recording shows for a slider that is
  applied. One pair of recordings; a repeat would firm the figure up.
- **The Noican Microphone device has a volume control and a mute of its
  own** (System Settings › Sound › Input, with the Noican Microphone
  selected; Audio MIDI Setup shows the same controls). The
  BlackHole-derived driver applies this one value (−64…0 dB) to every
  sample it loops, so it attenuates what *every* consumer receives. It
  was the cause of the owner's "quiet" report: the slider sat at about
  −35 dB. Who moved it is unknown — a user, or a meeting application's
  "automatically adjust microphone volume" feature writing the selected
  input device's system volume. Noican **does not** restore it (that
  would fight such an app and take away the user's own adjustment); it
  detects the condition and says so (below).
- **Where to adjust level, then:** the Noican Microphone slider (which
  is what consumers hear), or the microphone itself (MV7i: MOTIV Mix's
  Auto Level / gain; an interface's preamp). The microphone's *system*
  slider is not in the chain.

**Detection of a turned-down or muted virtual output.** The app reads
the Noican Microphone device's volume scalar and mute (input scope,
output scope as fallback; devices without the controls are not judged)
when an engine start settles and on every 1 Hz health-poll tick. Below
unity (scalar < 0.999) shows one orange line under the mode control —
"Noican Microphone volume is turned down in System Settings › Sound ›
Input — apps will hear you quietly." — and mute shows "Noican
Microphone is muted in System Settings › Sound › Input."; a nominal
reading clears it within a second. Each detection and resolution is
also written to the unified log (subsystem `com.lightsound.noican`,
category `engine-diagnostics`, prefix `Virtual output level:`) with the
scalar reading — one line per *distinct* reading, so every slider move
logs but a restart with the slider still down does not — so the
frequency of unexplained changes can be established over time. Nothing
is written back to the device.

**Isolating "Noican sounds quiet".** Record the same sentence at the
same distance twice in QuickTime (File › New Audio Recording, maximum
quality), once with the Noican Microphone selected and once with the
microphone directly, with Passthrough at 100% and both the Noican
Microphone slider and the microphone's slider at maximum. Convert and
measure each file:

```bash
afconvert -f WAVE -d LEI16 in.m4a out.wav
```

```bash
# Per-channel RMS/peak of a 16-bit WAV (reads the RIFF chunks directly:
# Python's wave module rejects the WAVE_FORMAT_EXTENSIBLE header afconvert
# writes).
python3 - "$WAV" <<'EOF'
import sys, struct, math
raw = open(sys.argv[1], "rb").read()
assert raw[:4] == b"RIFF" and raw[8:12] == b"WAVE", "not a WAV file"
pos, ch, bits, data = 12, None, None, None
while pos + 8 <= len(raw):
    cid, size = raw[pos:pos+4], struct.unpack("<I", raw[pos+4:pos+8])[0]
    body = raw[pos+8:pos+8+size]
    if cid == b"fmt ":
        ch, bits = struct.unpack("<H", body[2:4])[0], struct.unpack("<H", body[14:16])[0]
    elif cid == b"data":
        data = body
    pos += 8 + size + (size & 1)
assert ch and bits == 16 and data is not None, f"unexpected format: ch={ch} bits={bits}"
samples = struct.unpack("<%dh" % (len(data) // 2), data)
print(f"channels: {ch}")
for c in range(ch):
    s = samples[c::ch]
    rms = math.sqrt(sum(x * x for x in s) / len(s)) / 32768
    peak = max(abs(x) for x in s) / 32768
    db = 20 * math.log10(rms) if rms > 0 else float("-inf")
    print(f"ch{c}: rms {db:6.1f} dBFS  peak {peak:.3f}  ({'SILENT' if peak < 0.001 else 'signal'})")
EOF
```

Read the two outputs together:

1. Noican channels differ from each other (one `SILENT`, or a gap
   larger than 0.1 dB): a routing regression — the aggregate path must
   be dual mono on this build; check the `Aggregate output routing` line
   and file it.
2. Noican channels equal, both far below the direct recording, and the
   popover shows the turned-down/muted line: the Noican Microphone
   slider. Raise it in System Settings and re-record; if it drops again
   without your doing, note which meeting app was running (the log's
   `Virtual output level:` lines carry the scalar and the time).
3. Noican channels equal, no notice shown, and still quieter than the
   direct recording by about the difference the microphone's system
   slider makes: the direct recording was taken with that slider up,
   which Noican does not apply — set the level at the microphone or on
   the Noican Microphone slider instead. Verify by moving the
   microphone's slider and re-recording through Noican: the level must
   not follow.
4. Both recordings equally quiet: the microphone or the room; not
   Noican.

Then, with the engine running on the Noican Microphone:

5. Move the Noican Microphone slider to about 50%: within one second
   the orange line appears under the mode control and the log gains a
   `Virtual output level:` warning with a scalar around 0.5. Move it
   back to maximum: the line disappears within a second and the log
   records the resolution. Repeat with the mute checkbox (Audio MIDI
   Setup shows one for the device): the mute wording appears and
   clears the same way. The level itself must not move on its own at
   any point — Noican never writes it.

## Preview (self-monitor)

Preview mode runs the engine and additionally plays the processed
microphone signal on the system default output device through a second,
output-only AUHAL fed by a dedicated monitor ring. It shares no state
with the meeting-facing path except the lock-free tee in the inference
worker. Preview and On both feed the virtual microphone; switching
between them only arms or disarms the monitor.

1. Connect wired headphones and make them the system default output.
2. Select Preview in the menu (directly from Off, or from On).
3. Speak: the processed voice must be audible with a modest constant
   delay (engine latency plus ~40 ms of monitor ring priming). The delay
   is by design, not a defect. The status line reads
   `Previewing`.
4. Headphones are mandatory: through speakers the processed microphone
   feeds back into itself (Phase 0/1 has no AEC). There is deliberately
   no persistent warning text — unsafe outputs are refused on press with
   the reason shown, and the feedback guard explains itself when it
   trips.
5. Switch models while previewing: the voice must keep playing across
   the switch with only the bounded fade — no click, no full-scale
   burst, no dropout beyond the fade.
6. Switch Preview → On: playback must stop immediately; the engine keeps
   running without interruption.
7. While recording from the virtual device in QuickTime, switch between
   Preview and On: the recording must be unaffected.
8. Set the system default output to each of the following and press
   Preview:
   - the BlackHole/Noican loopback (the preview would reach the meeting
     twice),
   - a Multi-Output Device (Audio MIDI Setup) containing BlackHole (the
     aggregate can hide the meeting loopback, and the feedback guard
     cannot catch that route),
   - the built-in speakers (the voice would feed straight back into the
     microphone).
   The press must be refused in place: the mode and the engine (whether
   Off or On) stay exactly as they were — Off never starts the engine —
   and one short line ("Preview needs headphones — <cause>.") explains
   the reason under the control, without device UIDs. With the message
   showing, switch the default output back to headphones: the message
   must clear within about a second, and pressing Preview must then
   work.
9. A monitor failure at runtime (one that passed the pre-flight check),
   including a feedback-guard trip: the pill stays on Preview with a red
   warning tint, the engine keeps running (status returns to
   `Running`), and the reason renders under the control.
   Re-tapping Preview retries the monitor.
10. Select Off, then Preview again: the preview must come back cleanly
    with no stale audio replayed and no double playback.
11. Compare % CPU in Activity Monitor between On and Preview: the
    increase must be small (the monitor path only copies samples).
12. The monitor clock is not drift-corrected: over long previews an
    occasional short gap (underrun re-prime) or discarded block
    (overrun) is acceptable; persistent crackle is not.
13. *(Optional, external speakers required)* With USB/Bluetooth speakers
    as the default output — which the device-type check cannot classify —
    select Preview and raise the volume until feedback starts: within
    about half a second of sustained near-clipping output the feedback
    guard must silence the preview on its own; the pill stays on Preview
    with the red warning tint, the engine keeps running, and the menu
    explains why. Re-tapping Preview re-arms the guard and the monitor.
14. **Headphone jack unplug** *(wired headphones in the built-in jack)*:
    with the jack as the default output, select Preview, then unplug the
    headphones while it plays. The preview must stop itself within about
    a second — before anything audible comes out of the internal
    speakers beyond a moment of bleed — with the pill staying on Preview
    in the red warning tint, the engine still running (status returns to
    `Running`), and "Preview stopped: …" under the control. Two
    machine-dependent paths must both land here: on most Macs the same
    built-in device flips its data source from `'hdpn'` to `'ispk'`
    (caught by a data-source listener on the monitor's own device, plus
    the 1 Hz health poll as backstop); on machines where the jack is a
    separate device, the device disappears (caught by the device-list
    path, with a "device was disconnected" reason instead). Plug the
    headphones back in and re-tap Preview: the monitor must come back on
    the vetted output.

Changing the default output while previewing does not retarget the
monitor in this version; switch to On and back to Preview to pick up the
new device. The *safety* of the device the monitor actually plays on is,
however, watched continuously while the preview plays (step 14): the
enable-time-only vetting was a known hole where unplugging the jack let
the refused internal speakers keep playing with only the feedback guard
as insurance. Note the watcher re-vets the monitor's own device via
`noican_monitor_device_error`, not `noican_monitor_target_error` — the
latter judges the *current default output*, which may already have moved
elsewhere while the monitor stays on the old device.

## Level meters

The inference worker publishes per-block (10 ms) input (pre-model) and
output (post-model) peak levels with a short exponential decay; the menu
polls them at ~20 Hz only while the popover is open. The meters draw on
a shared −60…0 dB scale and are shown only while the engine runs.

1. Select On (or Preview) and open the popover.
2. Speak: the input bar must move with your voice, and the output bar
   must follow while you speak.
3. Stay silent with steady noise present (fan, keyboard typing): the
   output bar must sit clearly below the input bar — visual confirmation
   that noise suppression is working without listening to the stream.
4. Switch models while watching: the bars must not spike to full scale,
   freeze, or oscillate wildly (only the bounded switch fade).
5. The meters must move identically in Preview and On.
6. Select Off: the monitoring section disappears (and reappears at zero
   on the next start).
7. Close the popover and watch the app in Activity Monitor for a minute:
   CPU use and wake-ups must drop back to idle. The menu content (and
   with it the 20 Hz level poll) is built when the status-item popover
   opens and torn down when it closes — this check catches a lifecycle
   regression on the tested OS version.

## Settings persistence

The app persists the selected microphone UID, model id, and strength in
`UserDefaults` and restores them on the next launch through a reducer
event. The mode is deliberately never restored: the app always launches
Off, so starting it never captures the microphone (TCC prompts and live
capture must follow a user action). The login-item state is not
persisted by the app — `SMAppService` itself is the source of truth and
is re-read at launch.

1. Select a non-default microphone (e.g. a USB microphone), a
   non-default model (e.g. `DPDFNet2 48k HR`), and a non-default
   strength (e.g. 60%). Quit and relaunch the app.
2. The microphone checkmark, the Model picker, and the strength slider
   must show the persisted values; the mode must be **Off** and no
   microphone capture may start (no TCC prompt, no aggregate device in
   Audio MIDI Setup, no level movement).
3. Select On: the engine must start with exactly the restored
   microphone, model, and strength.
4. Unplug the USB microphone, relaunch, and confirm the selection falls
   back to another device while the app stays usable. Plug the USB
   microphone back in, relaunch again: the stored selection must return
   (a temporarily missing device must not overwrite the preference).
5. While running, switch to a model whose weights are not downloadable
   (e.g. disconnect the network and pick an unfetched model): after the
   failure reverts the picker, relaunch — the picker must restore the
   model that was actually running, not the failed pick.

## Launch at login

Registration uses `SMAppService.mainApp` and depends on the app's
location and signature; development builds run from `dist/` commonly
cannot register. CI can only compile this path — every claim below is
hardware-only. Test with the app copied to `/Applications` (Developer ID
build) for the positive path.

1. Toggle "Start at login" on in the menu footer. The toggle must
   reflect the real outcome: on success it stays on and the app appears
   under System Settings → General → Login Items; on failure it snaps
   back off with the reason shown under the toggle (no silent failure,
   no toggle stuck in a state `SMAppService` does not report).
2. Log out and back in (or reboot): the app must launch into the menu
   bar, mode Off, without capturing the microphone.
3. Toggle it off: the entry must disappear from Login Items, and after
   a re-login the app must not start.
4. *(Development build from `dist/`)* Toggle it on: if registration
   fails, the failure must be visible — toggle back off plus the error
   text — rather than pretending success.

## Strength control

The strength slider (0–100%, default 100%) blends the processed signal
(wet) with the raw microphone (dry) inside the inference worker,
upstream of the output ring and the preview monitor tee — Preview and
the virtual microphone always hear the same mix. The dry path is
delay-compensated by the active model's reported latency, so a partial
mix must not comb-filter or double the voice. The value crosses to the
engine as one atomic: moving the slider never rebuilds the transport.

1. Select On with a noisy source present (fan, typing) and record from
   the virtual device; watch the level meters.
2. At 100%: unchanged Phase 0/1 behavior (full suppression).
3. Drag toward 0% while speaking: background noise must fade back in
   smoothly — no clicks, no zipper steps, no dropouts, and the engine
   must not restart (status stays `Running`, meters keep moving, no
   busy spinner).
4. At 0%: the recording must be the (delay-aligned) raw microphone.
5. At 50%, speak continuously: the voice must sound like one voice, not
   a doubled/hollow "comb-filtered" voice. If any model still sounds
   comb-filtered at 50%, record which model — its reported
   `latency_samples()` (resampling + frame buffering + declared
   algorithmic delay) may underestimate its true end-to-end delay; the
   mismatch, not the mixer, is the bug to file.
6. Repeat 3–5 in Preview mode with headphones: the audible mix must
   track the slider exactly like the recording does.
7. Switch models while at 50%: only the bounded switch fade, no click
   from the dry-compensation delay jumping between the models'
   different latencies (the jump lands inside the silent fade
   boundary).
8. Relaunch: the slider must restore its persisted position.

## Real-time audit

Run Instruments with Allocations, System Trace, and Thread Sanitizer in
separate runs:

1. Identify the AUHAL render callback.
2. Confirm no heap allocation, Objective-C/Swift ARC traffic, mutex wait,
   file I/O, logging, or blocking syscall occurs in that callback.
3. Confirm the callback only calls `AudioUnitRender`, moves `f32` samples
   through the preallocated SPSC rings, and signals the worker's dispatch
   semaphore (a non-blocking call).
4. Confirm `noican-inference` joins the AUHAL `os_workgroup`. A failed join
   sets the engine fault flag and fails this test.
5. Confirm the `noican-inference` worker blocks between device callbacks:
   over a minute of engine-on idle time its CPU use must be far below one
   core (it waits on the semaphore; a busy-spinning worker fails this test).
6. Induce inference overload with the heaviest model. The callback must emit
   silence on output-ring underrun rather than block.

## Output-underrun diagnostics (real-time budget)

A worker that misses its 10 ms block budget drains the output ring;
the virtual-microphone callback then zero-fills — audible as dropouts
and a lower average level in recordings — while Preview masks it
behind the monitor ring's re-priming cushion. The engine counts these
events on both transports: output callbacks that were fully starved —
zero real samples available for an entire I/O period (the start-up
ramp and benign partial shortfalls from 480-sample block quantization
are excluded by design) — and the worker's per-block processing
times (total blocks / blocks over 10 ms / maximum). Counters reset on
engine start and on every model switch, so readings are attributable
to the active model. x86-64 measurements already show FastEnhancer-L
over budget on ~7% of blocks (docs/tech-research.md §5.2 suggests
DeepFilterNet3 is also near the budget on Apple Silicon); this
procedure produces the on-device evidence.

The counters surface in the unified log — no popover UI by design
(they are a diagnosis tool, not a user control). In Console.app,
filter subsystem `com.lightsound.noican` (category
`engine-diagnostics`), or stream in a terminal:

```bash
log stream --predicate 'subsystem == "com.lightsound.noican"' --level info
```

One warning line appears for each 1 Hz health-poll tick in which the
underrun count grew, carrying the count, the active model id, and the
worker block statistics. In addition, one info line appears about a
second after every engine start — `Engine transport diagnostics:
worker realtime scheduling <bool>, Rosetta-translated process <bool>`
— reporting whether the inference worker's mach time-constraint
promotion succeeded and whether the process runs translated. Both must
read `true`/`false` respectively; a `false` realtime flag or a `true`
translation flag means the budget numbers measure scheduling or
translation overhead, not model cost (the first hardware run,
[2026-09-02](acceptance/2026-09-02-underrun-baseline.md), showed
exactly that failure mode before the worker was promoted: chronic
41–49% budget misses on FastEnhancer-L and one-shot 40 ms stalls even
on light models).

1. Select On with a 48 kHz microphone (aggregate path) and record from
   the virtual device throughout. Confirm the transport line reads
   `worker realtime scheduling true` and
   `Rosetta-translated process false`.
2. For each of `FastEnhancer-B 48k` and `DPDFNet2 48k HR` (light
   controls), then `DPDFNet8 48k HR`, `DeepFilterNet3 48k`, and
   `FastEnhancer-L 48k` (suspects): select the model, speak
   continuously for at least 60 seconds, and note every diagnostic
   line (or its absence).
3. Pass criteria for the controls: **no underrun line at all** for
   FastEnhancer-B (and the other light models) — a nonzero count on a
   light model is a false positive and fails this check.
4. For the suspects, record the counts verbatim (model, underruns,
   over-budget blocks / total blocks, max ms) into the result record.
   These numbers decide the countermeasure phase; do not tune anything
   from guesses.
5. Cross-check audibly: models that logged underruns must be the same
   ones whose recordings stutter; the recording of a model with zero
   underruns must be free of dropouts.
6. Repeat step 2 for one suspect on the split transport (a Bluetooth
   or 44.1 kHz microphone) to confirm the counter works there too. Do
   not compare split counts against aggregate counts numerically: the
   split ring is primed with a ~50 ms cushion the drift servo then
   maintains, so a single split underrun means the worker fell behind
   by the whole cushion — a far more severe event than one aggregate
   underrun, which only needs the shallow block-phase reservoir to run
   dry.

## Clock drift and endurance

The physical microphone and virtual output use different clocks; this is the
acceptance test for Aggregate Device drift compensation.

1. Use a physical USB microphone where possible, because its clock is
   clearly independent from BlackHole.
2. Run a continuous two-hour recording through Noican.
3. Speak or play a short reference tone every five minutes.
4. Inspect the entire file for discontinuities and measure reference-tone
   spacing.
5. Pass criteria:
   - no periodic click, duplicate block, or dropped block,
   - no increasing timing error,
   - no engine fault,
   - bounded memory use,
   - Aggregate Device remains alive.
6. Repeat sleep/wake, microphone disconnect/reconnect, and app quit/relaunch.
   Resource teardown must not leave a visible or reusable stale aggregate.

Record this result separately for each macOS major version under support,
especially macOS 26.

## Acceptance checklist (Phase 0 hybrid build)

The five transport items that candidate B passed, plus the two items new to
this build:

1. **Running status**: selecting On (with mic permission granted) reaches
   `Running` with the green indicator.
2. **Audio reaches recordings**: a QuickTime/OBS recording from the virtual
   device contains the processed microphone signal.
3. **Continuity**: no dropouts, periodic clicks, or runaway latency over a
   30-minute session.
4. **Switching stability**: live model switches across all listed models
   produce no crash, no blowup, only the bounded fade.
5. **Clean stop**: selecting Off tears down AUHAL and the private aggregate;
   nothing stale remains in Audio MIDI Setup; the app quits cleanly.
6. **16 kHz models are audible** *(new)*: Hush and UL-UNAS produce clearly
   audible, intelligible speech in the live path.
6b. **Hush loudness parity** *(2026-08-31 fix)*: at strength **100%**
   (partial strength dilutes the comparison through the shared dry
   path), Hush's perceived speech loudness matches the other models
   (functional test 10b) — the measured makeup gain closed its
   −3.4 dB to −1.5 dB voiced-RMS deficit, and no clipping artifacts
   appear on loud speech.
7. **Full model list** *(new)*: the Model picker shows every `main` registry
   stage (Passthrough, FastEnhancer T/B/S/M/L, DPDFNet2/8, DeepFilterNet3,
   UL-UNAS, Hush, and TSE marked "requires enrollment").

## Acceptance checklist (preview + level meters)

Run the Preview and Level meters procedures above; the build passes when:

1. **Mode control**: Off / Preview / On transitions all work in both
   directions; Preview ↔ On switches are instant and never interrupt the
   virtual-microphone path.
2. **Preview audible**: in Preview mode, your own processed voice is
   heard on headphones with a small constant delay; switching to On
   stops it immediately.
3. **Switching under preview**: model switches while previewing produce
   no dropout beyond the bounded fade and no full-scale burst.
4. **Main path isolation**: a QuickTime recording from the virtual
   device is unaffected by Preview ↔ On switches.
5. **Unsafe output refusal**: pressing Preview while the default output
   is the BlackHole/Noican loopback, a Multi-Output/aggregate device, or
   the built-in speakers is refused in place — mode and engine
   unchanged, the reason shown under the control — and the message
   clears within about a second of a safe output returning.
6. **Feedback guard** *(optional; needs external speakers)*: sustained
   feedback through an unclassifiable output stops the preview by itself
   within ~1 s; the pill stays on Preview with the red warning tint and
   the menu explains why.
6b. **Jack-unplug auto-stop**: unplugging wired headphones from the
    built-in jack while previewing stops the preview by itself within
    about a second with "Preview stopped: …" under the control; the pill
    stays on Preview with the red warning tint and the engine keeps
    running. On machines whose jack is a separate device, the
    device-loss path must produce the same result with a
    "device was disconnected" reason.
7. **Restart coherence**: Off → Preview brings the preview back cleanly,
   with no stale audio and no double playback.
8. **Intent is never moved**: on any failure (start failure, device
   loss, monitor failure, feedback trip) the pill keeps the user's
   selection with a red warning tint and the reason below the control;
   re-tapping the segment retries, and selecting a working microphone
   restarts into the selected mode automatically.
9. **Preview cost**: % CPU does not increase materially in Preview mode.
10. **Input meter follows speech**: the input bar moves when you speak.
11. **Suppression visible**: during noise-only passages (fan, typing)
    the output bar sits clearly below the input bar.
12. **Meter stability**: meters do not spike or freeze across model
    switches; the monitoring section is hidden while Off (and while a
    failure is shown) and returns at zero on the next start.
13. **Microphone switching**: changing the microphone while running
    rebuilds the transport with the same model and mode after a brief
    gap; hot-plugged devices appear in the list automatically.
14. **Mode-control animation**: the sliding pill stays visually intact
    while multi-line status/error text appears and disappears around it.
15. **No transitional flash**: pressing a pill while a failure is shown
    never flashes optimistic UI — no momentary blue pill, no meters
    sliding in and out, no error text blinking away and back. The view
    changes once, when the attempt settles (sections, colors, and error
    text render from the last settled state; the spinner and the status
    line are the only transitional feedback).

## Acceptance checklist (Phase 2 controls)

Run the Settings persistence, Launch at login, and Strength control
procedures above; the build passes when:

1. **Selections survive a relaunch**: microphone, model, and strength
   restore from `UserDefaults`; a temporarily unplugged microphone does
   not lose its stored preference.
2. **Always launches Off**: no mode restoration, no TCC prompt, no
   capture, and no aggregate device at launch — in every persistence
   scenario, including launch-at-login starts.
3. **Login item registers honestly**: the toggle shows the re-read
   `SMAppService` status; a failed registration snaps the toggle back
   with the reason under it (never a silent failure or an optimistic
   toggle).
4. **Login item round-trips**: on (from `/Applications`) → appears in
   Login Items and auto-starts after re-login; off → disappears and
   stays stopped.
5. **Strength blends without artifacts**: smooth dry/wet transition
   across the whole slider range while running — no clicks, zipper
   noise, restarts, or busy states.
6. **No comb-filter at partial strength**: at 50% the voice stays
   single (dry path delay-compensated) on every listed model.
7. **Preview parity**: the preview monitor plays the same mix the
   virtual microphone receives at every slider position.
8. **Strength × switching**: model switches at partial strength produce
   only the bounded fade, with no click from the latency change.

### Acceptance record (Phase 2 controls, 2026-08-27, owner-run)

Run against the PR #14 build (`ad7defd`) on Apple hardware:

- **Settings persistence (A1–A5): pass** — all five checks, including the
  unplugged-microphone preference survival and the failed-switch revert.
- **Launch at login (B1–B4): functional pass** — registration, re-login
  round-trip, and failure surfacing all behaved. Design follow-up: the
  switch-style toggle was visually overweight for a footer setting and
  was restyled to a checkbox afterwards.
- **Strength control: pass except C5** — smooth blending, preview
  parity, persistence, and switching all passed. **C5 failed on DPDFNet2
  and DPDFNet8** (doubled voice at 50%); every other model passed. Root
  cause per the procedure's own diagnosis: both DPDFNet profiles carry
  4 hops (1920 samples, 40 ms) of architectural lookahead their ONNX
  metadata does not report, so `output_delay()` under-reported and the
  dry path ran 40 ms early. Fixed by a measurement-backed lookahead
  constant (cross-correlation of real speech against the aligned CLI
  output now measures 0 residual lag on both profiles). **C5 re-verified
  on hardware 2026-08-28 (owner-run, PR #15 build): the doubled voice at
  50% strength is resolved on both DPDFNet2 and DPDFNet8.**
- Observation, not a defect: transient noises (trackpad/keyboard clicks)
  pass through some models at 100% strength — click suppression is a
  property of each model (DPDFNet8, DeepFilterNet3, and Hush removed
  clicks; the lighter models let them through). Lowering the strength
  mixes raw-microphone clicks back in on every model, by design.

## Acceptance checklist (native-rate capture, issue #7)

Run the non-48 kHz microphone procedure above; the build passes when:

1. **Running on a native rate**: a microphone that cannot run at 48 kHz
   — a Bluetooth headset on HFP (16 kHz) *or* a 44.1 kHz-family device
   — can be selected and the engine reaches `Running`. Record which
   rate was exercised; both kinds are in scope, but one device proves
   the transport.
2. **48 kHz output**: recordings through the virtual device remain
   48 kHz and intelligible (telephony bandwidth expected at a telephony
   source; full-band expected from a 44.1 kHz source).
3. **No drift artifacts**: a 30-minute session produces no clock-drift
   clicks, gaps, pitch wobble, or accumulating timing error — on a
   44.1 kHz device this also proves the servo at a non-integer ratio.
4. **Aggregate path unchanged**: the built-in / USB 48 kHz microphone
   path behaves exactly as before (behavior, quality, latency).
5. **Strength alignment**: 50% strength on the split path produces a
   single voice (no comb-filter/double voice).
6. **Rate-change recovery**: an A2DP ↔ HFP renegotiation while running
   rebuilds automatically (or passes through unaffected); it never
   leaves a dead session. (Bluetooth devices only; a fixed-rate
   44.1 kHz interface has nothing to renegotiate.)
7. **UI truthfulness**: the native rate shows in the microphone list in
   audio notation, the notice for the selection matches the device kind
   (telephony trade-offs vs. plain conversion), and devices outside
   8–192 kHz (or with an unreadable rate) are still refused with a
   clear reason.
8. **Real-time constraints hold**: re-run the Real-time audit on the
   split transport — both callbacks (capture and virtual output) stay
   allocation- and lock-free; resampling runs on the inference worker
   with buffers preallocated at start (no growth in the worker after
   the first second).
9. **Split-path underruns**: with FastEnhancer-B the split transport
   logs zero underruns over 60+ s of continuous speech (this also
   closes criterion 4 of the output-underrun checklist below).

## Acceptance checklist (composite input/output microphone)

Run the composite input/output microphone procedure above; the build
passes when:

1. **Recordings carry audio**: with a headphone-equipped USB microphone
   (or an interface with outputs) as the input, a QuickTime recording
   from the Noican virtual microphone contains the processed speech —
   not silence. Record the device, its input/output channel counts, and
   the aggregate's output channel count.
2. **Nothing leaks to the microphone's own outputs**: Noican's output
   is inaudible on headphones plugged into the microphone's own jack,
   in both On and Preview.
3. **Dual mono, level pinned**: the same recording with the built-in
   microphone carries the same signal on every virtual-microphone
   channel (per-channel RMS within 0.1 dB of each other), and channel
   0's RMS is within ±1 dB of a same-settings recording on the previous
   build (per-channel measurement with the "Level integrity" script) —
   the engine level is unchanged, only the silent channel is gone. The
   composite device must show the same shape.
4. **Everything else as before**: Preview, model switching, meters, and
   the underrun diagnostics behave exactly as on the earlier records
   with the composite device selected.
5. *(Optional)* **Split path unaffected**: a recording through a
   native-rate (Bluetooth or 44.1 kHz) microphone still carries audio —
   the split transport has no aggregate and takes no channel map.

## Acceptance checklist (level integrity)

Run the Level integrity procedure above; the build passes when:

1. **Dual mono on the aggregate path**: with the built-in microphone
   (Passthrough, 100%, Noican Microphone slider at maximum) the
   virtual-microphone recording carries the same signal on every
   channel (RMS within 0.1 dB), and channel 0's RMS is within ±1 dB of
   a same-settings recording on the previous build.
2. **Dual mono on a composite device**: the same with the MV7i (or
   another input/output device), and the `Aggregate output routing`
   line shows the one-to-one map (`[-1, -1, 0, 1]` for a stereo
   headphone jack) read back unchanged after initialize.
3. **Both ears**: the virtual microphone's signal, played back from a
   recording or monitored through a consumer app, is heard in both
   ears of a pair of headphones.
4. **Turned-down/muted notice**: moving the Noican Microphone slider to
   50% shows the notice within one second and moving it back to
   maximum clears it; mute behaves the same with its own wording; the
   log carries each transition with the scalar; the level is never
   changed by Noican.
5. **Everything else as before**: Preview, model switching, and the
   underrun diagnostics (zero on a light model) behave exactly as on
   the earlier records.
6. **Split path level**: a Bluetooth (or other native-rate) headset
   recording is unchanged (both channels, same level as before), and
   two additional recordings through Noican — the headset's *system*
   input slider at the middle and at maximum — settle whether the
   split transport applies that slider. Record the two RMS values and
   the conclusion, and keep the Level integrity section's second
   bullet in step with the accumulated evidence (first pair recorded
   2026-09-05: +2.2 dB, attributed to speech variance).

## Acceptance checklist (output-underrun diagnostics)

Run the Output-underrun diagnostics procedure above; the build passes
when:

0. **Worker is real-time**: the engine-start transport line reads
   `worker realtime scheduling true` and
   `Rosetta-translated process false`.
1. **No false positives**: light models (FastEnhancer-B and friends)
   log zero underruns over 60+ seconds of continuous speech on the
   aggregate path.
2. **Counts recorded**: FastEnhancer-L, DeepFilterNet3, DPDFNet8 (and
   any other suspect) have their underrun and block-time numbers
   recorded verbatim in the result record.
3. **Counts match ears**: models that log underruns are exactly the
   models whose virtual-microphone recordings stutter.
4. **Split transport covered**: at least one model's counters were
   exercised through a native-rate (Bluetooth or 44.1 kHz) microphone.
5. **No regression**: recordings, meters, preview, and model switching
   behave exactly as before on models with zero underruns.

## Result record

For each run, retain:

- Mac model and chip,
- macOS and Xcode versions,
- app and driver commit IDs,
- app and driver signature output,
- physical input and virtual device UIDs,
- selected model,
- sample rate and buffer size,
- test recording,
- pass/fail for the acceptance checklist, callback audit, and endurance,
- every unverified or failed item.

Hardware acceptance is complete only when this record contains evidence for
every check rather than an inferred result.
