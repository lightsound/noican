# macOS Build and Hardware Test Plan

## Verification boundary

The Linux CI covers the common engine, real ONNX inference, multi-format CLI
input (WAV/AIFF/AIFC/CAF/M4A), lock-free model switching, and the C ABI. The
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
- A 48 kHz-capable microphone. Bluetooth headset microphones run on
  telephony profiles (8/16/24 kHz) and are rejected with a clear error in
  Phase 0 — Bluetooth *playback* is fine, only the input side is limited
  (tracked in issue #7). The built-in microphone works.

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
   - the Model selector (below the Microphone list): **every registry
     stage** as always-visible rows with a checkmark on the selection
     and a trailing purpose tag (e.g. "balanced default") — Passthrough,
     FastEnhancer T/B/S/M/L, DPDFNet2, DPDFNet8, DeepFilterNet3,
     UL-UNAS, Hush, and TSE Conv-TasNet 48k disabled as "requires
     enrollment". Hovering a row pops the model's profile card out
     beside the menu after a short delay: name, tag, four dot ratings
     (Noise removal / Voice quality / Responsiveness / Efficiency, all
     "more is better"), and the raw facts (native rate, measured delay,
     size). Once up, the card must **stay up while the pointer moves
     between rows, following the hovered row's position and swapping
     its content in place** (no per-row blink or re-present animation),
     hide shortly after the pointer leaves the rows, and — critically —
     hovering must never close the menu popover itself.
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
3. Select a 48 kHz-incapable device (e.g. a Bluetooth headset
   microphone) while Off, then select On: the refusal must be
   **instant** (a pre-flight reads the device's advertised rates — no
   busy spinner, no teardown), with a clear reason under the mode
   control and the pill **staying on On** with a red warning tint (the
   control shows the user's intent; the system never moves it). Then
   select the built-in microphone: the engine must restart automatically
   into the selected mode and reach Running. Re-tapping the red segment
   must also retry.
4. While running, click the incapable device in the list: the switch is
   refused in place — the checkmark returns to the working microphone,
   the reason appears under the list, and the engine keeps running
   uninterrupted.
5. If a live switch fails at runtime (a failure the pre-flight cannot
   see, e.g. the new device vanishing mid-switch), the app must fall
   back to the previous microphone automatically — one rebuild attempt,
   reason under the list — instead of leaving the session dead.

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
   and "Preview is unavailable: …" explains the reason under the
   control. With the message showing, switch the default output back to
   headphones: the message must clear within about a second, and
   pressing Preview must then work.
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
   CPU use and wake-ups must drop back to idle. The 20 Hz level poll is
   bound to the popover view's lifetime, but `MenuBarExtra(.window)` has
   kept hidden content views alive on some macOS releases — this check
   catches that regression on the tested OS version.

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
  output now measures 0 residual lag on both profiles). **C5 must be
  re-verified on hardware for DPDFNet2/8 after that fix.**
- Observation, not a defect: transient noises (trackpad/keyboard clicks)
  pass through some models at 100% strength — click suppression is a
  property of each model (DPDFNet8, DeepFilterNet3, and Hush removed
  clicks; the lighter models let them through). Lowering the strength
  mixes raw-microphone clicks back in on every model, by design.

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
