# macOS Build and Hardware Test Plan

## Verification boundary

The Linux CI covers the common engine, real ONNX inference, multi-format CLI
input (WAV/AIFF/AIFC/CAF/M4A), lock-free model switching, and the C ABI. The
macOS CI job additionally covers clippy/tests for `aarch64-apple-darwin`
(including the AUHAL transport), SwiftLint in strict mode, and the release
app build with `swift -warnings-as-errors`.

Physical microphone capture, TCC prompts, aggregate clock behavior,
BlackHole routing, audible switching, 16 kHz-model audio quality, and
long-session stability require Apple hardware. **They are not claimed as
verified until every applicable check below is recorded.** The transport
design passed this plan on macOS 26 / Apple Silicon in its candidate-B
incarnation; the hybrid build (C engine + B transport) must be re-accepted.

## Prerequisites

- Apple Silicon Mac running macOS 14.2 or newer.
- Xcode command-line tools.
- Rust 1.98.0 with the `aarch64-apple-darwin` target.
- Swift 6.1 or newer and SwiftLint.
- A Developer ID Application identity for a distributable app build.
- Stock BlackHole 2ch for Phase 0, or the separately built and
  Developer-ID-signed Noican BlackHole fork (Phase 1).
- Headphones. Phase 0 has no AEC and must not be evaluated through speakers.

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

1. Install the virtual device (stock BlackHole 2ch for Phase 0).
2. Restart the audio daemon. On macOS 26, SIP rejects
   `sudo launchctl kickstart -k system/com.apple.audio.coreaudiod`; use:

   ```bash
   sudo killall coreaudiod
   ```

3. Open Audio MIDI Setup and confirm a two-channel 48 kHz virtual device
   appears.
4. Confirm the device has both output and input streams and can loop a test
   signal before involving Noican.
5. For a self-built driver (Phase 1 and later), record the bundle signature:

   ```bash
   codesign --verify --deep --strict --verbose=2 \
     "/Library/Audio/Plug-Ins/HAL/<driver-name>.driver"
   ```

Do not disable SIP or use an ad-hoc driver signature for the acceptance test.

## Functional test

1. Connect wired headphones.
2. Launch `dist/Noican.app`.
3. Confirm the menu bar popover shows:
   - the status header with a state indicator,
   - the Noise Cancellation toggle,
   - the Microphone picker listing physical inputs,
   - the Model picker listing **every registry stage**: Passthrough,
     FastEnhancer T/B/S/M/L, DPDFNet2, DPDFNet8, DeepFilterNet3, UL-UNAS,
     Hush, and TSE Conv-TasNet 48k marked "requires enrollment".
4. Select a physical microphone and `FastEnhancer-B 48k`.
5. Enable the toggle and grant microphone access when macOS prompts.
6. Confirm status changes to `Running · FastEnhancer-B 48k`.
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
    "requires enrollment" status message, engine still running the previous
    model, picker reverted.
12. Disable the toggle. Confirm the private Aggregate Device disappears and
    the virtual microphone no longer receives new processed audio.

If the status reports an audio fault, collect the macOS version, selected
device UIDs, buffer size, and Console entries. Do not characterize the path
as working.

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

1. **Running status**: toggling on (with mic permission granted) reaches
   `Running · <model>` with the green indicator.
2. **Audio reaches recordings**: a QuickTime/OBS recording from the virtual
   device contains the processed microphone signal.
3. **Continuity**: no dropouts, periodic clicks, or runaway latency over a
   30-minute session.
4. **Switching stability**: live model switches across all listed models
   produce no crash, no blowup, only the bounded fade.
5. **Clean stop**: toggling off tears down AUHAL and the private aggregate;
   nothing stale remains in Audio MIDI Setup; the app quits cleanly.
6. **16 kHz models are audible** *(new)*: Hush and UL-UNAS produce clearly
   audible, intelligible speech in the live path.
7. **Full model list** *(new)*: the Model picker shows every `main` registry
   stage (Passthrough, FastEnhancer T/B/S/M/L, DPDFNet2/8, DeepFilterNet3,
   UL-UNAS, Hush, and TSE marked "requires enrollment").

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
