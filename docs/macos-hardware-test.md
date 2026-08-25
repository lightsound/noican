# macOS Build and Hardware Test Plan

## Verification boundary

The Linux checks cover the common engine, real ONNX inference, WAV output, lock-free model publication, C ABI, and an Apple-Silicon type check of the AUHAL crate. The final app link runs in GitHub Actions on a macOS runner.

Physical microphone capture, TCC prompts, aggregate clock behavior, BlackHole routing, audible switching, and long-session stability require Apple hardware. They are not claimed as verified until every applicable check below is recorded.

## Prerequisites

- Apple Silicon Mac running macOS 14.2 or newer.
- Xcode command-line tools.
- Rust 1.96.0 with `aarch64-apple-darwin`.
- Swift 6.1 or newer and SwiftLint.
- A Developer ID Application identity for a distributable app build.
- Stock BlackHole 2ch for Phase 0, or the separately built and Developer-ID-signed noican BlackHole fork.
- Headphones. Phase 0 has no AEC and must not be evaluated through speakers.

The BlackHole-derived driver is GPL-3.0 and remains a separate program. Do not add its source or object files to the application target.

## Build

```bash
rustup toolchain install 1.96.0 \
  --target aarch64-apple-darwin \
  --component clippy,rustfmt

cargo fmt --all --check
cargo clippy --locked \
  --target aarch64-apple-darwin \
  --package noican-coreaudio \
  --package noican-ffi \
  --all-features

swiftlint lint --strict --config .swiftlint.yml

# Ad-hoc local build:
bash scripts/build-macos-app.sh

# Developer ID build:
NOICAN_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
  bash scripts/build-macos-app.sh
```

Expected artifact: `dist/noican.app`.

Validate it:

```bash
codesign --verify --deep --strict --verbose=2 dist/noican.app
codesign --display --verbose=4 dist/noican.app
```

The app is a UI agent (`LSUIElement`) and therefore has no Dock icon.

## Driver check

1. Install the signed virtual driver under `/Library/Audio/Plug-Ins/HAL/`.
2. Restart `coreaudiod` using the driver project's installer procedure.
3. Open Audio MIDI Setup and confirm a two-channel 48 kHz virtual device appears.
4. Confirm the device has both output and input streams and can loop a test signal before involving noican.
5. Record the driver bundle signature:

   ```bash
   codesign --verify --deep --strict --verbose=2 \
     "/Library/Audio/Plug-Ins/HAL/<driver-name>.driver"
   ```

Do not disable SIP or use an ad-hoc driver signature for the acceptance test.

## Functional test

1. Connect wired headphones.
2. Launch `dist/noican.app`.
3. Confirm the menu bar item opens and shows:
   - an on/off toggle,
   - physical input devices,
   - every model slug,
   - status text.
4. Select a physical microphone and `fastenhancer-b`.
5. Enable the engine and grant microphone access when macOS prompts.
6. Confirm status changes to `Running · fastenhancer-b`.
7. In QuickTime, OBS, or a meeting app, select the BlackHole/noican virtual device as the microphone.
8. Record at least 30 seconds containing speech, steady fan noise, and keyboard noise.
9. Confirm the recording is non-silent, intelligible, and materially differs from a raw-microphone control.
10. Disable the engine. Confirm the private Aggregate Device disappears and the virtual microphone no longer receives new processed audio.

If the status reports an audio fault, collect the macOS version, selected device UIDs, buffer size, and Console entries. Do not characterize the path as working.

## Model switching

While recording one continuous file:

1. Switch among FastEnhancer T/B/S, DPDFNet2/8, DeepFilterNet3, UL-UNAS, and Hush.
2. Place markers immediately before each change.
3. Inspect the waveform around each marker at sample level.
4. Pass criteria:
   - no full-scale impulse,
   - no NaN/sustained digital noise,
   - only the bounded fade-to-silence/fade-in interval,
   - status reflects the selected model.

Model construction occurs on the control thread. The inference thread receives the fully prepared stage through a preallocated lock-free queue. The Core Audio callback never loads a model.

TSE requires a valid ECAPA enrollment and authenticated, checksum-confirmed model files as described in [model-assets.md](model-assets.md). It is excluded from this step until the upstream access and license blocker is resolved.

## Real-time audit

Run Instruments with Allocations, System Trace, and Thread Sanitizer in separate runs:

1. Identify the AUHAL render callback.
2. Confirm no heap allocation, Objective-C/Swift ARC traffic, mutex wait, file I/O, logging, or blocking syscall occurs in that callback.
3. Confirm the callback only calls `AudioUnitRender` and moves `f32` samples through the preallocated SPSC rings.
4. Confirm `noican-inference` joins the AUHAL `os_workgroup`. A failed join sets the engine fault flag and fails this test.
5. Induce inference overload with the heaviest model. The callback must emit silence on output-ring underrun rather than block.

## Clock drift and endurance

The physical microphone and virtual output use different clocks; this is the acceptance test for Aggregate Device drift compensation.

1. Use a physical USB microphone where possible, because its clock is clearly independent from BlackHole.
2. Run a continuous two-hour recording through noican.
3. Speak or play a short reference tone every five minutes.
4. Inspect the entire file for discontinuities and measure reference-tone spacing.
5. Pass criteria:
   - no periodic click, duplicate block, or dropped block,
   - no increasing timing error,
   - no engine fault,
   - bounded memory use,
   - Aggregate Device remains alive.
6. Repeat sleep/wake, microphone disconnect/reconnect, and app quit/relaunch. Resource teardown must not leave a visible or reusable stale aggregate.

Record this result separately for each macOS major version under support, especially macOS 26.

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
- pass/fail for switching, callback audit, and endurance,
- every unverified or failed item.

Hardware acceptance is complete only when this record contains evidence for every check rather than an inferred result.
