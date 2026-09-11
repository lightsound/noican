# Hardware test: Functional test, model switching, microphone switching

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

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
model files as described in [models.md](../models.md). It is excluded from this
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
   native-rate procedure, [native-rate-capture.md](native-rate-capture.md))
   — the Phase 0 refusal of telephony-rate
   devices and the later refusal of 44.1 kHz-family devices are both
   gone.
4. *(Requires a device whose rate lies outside the 8–192 kHz range the
   capture resampler converts, or whose rate is unreadable; real
   hardware like this is rare — skip when none is available. A
   44.1 kHz-only interface is **not** such a device any more: it takes
   the split path, see [native-rate-capture.md](native-rate-capture.md).)*
   Select such a device while
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
