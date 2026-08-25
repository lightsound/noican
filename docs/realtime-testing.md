# Real-Time Pipeline: Build & On-Device Test Procedure

Status: the real-time engine and menu-bar app **build** (verified in CI on
a macOS runner) but have **not been exercised on a physical Mac with real
audio devices**. Everything in the checklist below is unverified until you
run it. Known-unverifiable-in-CI items are marked ⚠.

## Prerequisites

1. macOS 14+ on Apple Silicon.
2. Rust 1.98 (`rustup toolchain install 1.98.0` — the repo pins it).
3. Xcode command-line tools (`xcode-select --install`).
4. **BlackHole 2ch** installed (stock, unmodified — Phase 0 uses it as-is):
   `brew install blackhole-2ch` or the installer from
   https://existential.audio/blackhole/. Reboot or restart `coreaudiod`
   if the device does not appear.
5. Model weights: from the repo root run
   `cargo run -p noican-cli --release -- fetch`
   and move/link the `models/` directory to
   `~/Library/Application Support/noican/models`
   (or launch the app with `NOICAN_MODELS_DIR` pointing at the repo's
   `models/` directory).

## Build

```sh
./scripts/build-macos-app.sh
open build/Noican.app
```

The script builds `noican-ffi` (Rust, static lib), the SwiftUI package
(`app/`), assembles `build/Noican.app`, and ad-hoc signs it.

## Smoke checklist (in order)

1. ⚠ **Menu bar icon appears**; the popover shows the Off toggle, an input
   device list containing your physical mic, and the model selector
   (passthrough + fetched models).
2. ⚠ **Mic permission**: toggling On the first time must show the
   microphone TCC prompt (from `NSMicrophoneUsageDescription`). Accept.
3. ⚠ **Passthrough loop**: with model = `passthrough`, toggle On. Open
   QuickTime Player → New Audio Recording → input = *BlackHole 2ch*; the
   level meter must follow your voice with barely noticeable delay.
   Status line shows `running: passthrough` and underruns stay at 0.
4. ⚠ **Denoise model**: switch the selector to `fastenhancer-s` (or
   `dpdfnet2`). Expect roughly one second of extra CPU while the model
   loads, then noise (fan, keyboard) audibly reduced in the QuickTime
   monitor. Speech must remain intact.
5. ⚠ **Click-free switching**: while speaking, switch between
   `passthrough` ↔ `fastenhancer-s` ↔ `dpdfnet2` repeatedly. The
   transition must be a short crossfade — no clicks/pops. (The engine
   swaps models lock-free on the inference thread; the audio callback is
   untouched.)
6. ⚠ **Meeting app**: select *BlackHole 2ch* as the microphone in
   Zoom/Meet and hold a test call.
7. ⚠ **16 kHz models**: `ul-unas` and `hush` run at 16 kHz behind the
   resampling adapter; verify they sound band-limited but clean.
8. ⚠ **Failure behavior**: selecting a model whose weights are missing
   must surface an error in the popover and keep the engine on the
   previous model. If a model crashes mid-run the engine fails **open**
   (raw mic passthrough) and the status line shows `MODEL FAILED`.
9. ⚠ **Drift/long-run** (docs/tech-research.md §13-4): leave the engine
   running ≥ 2 h into QuickTime; underruns should not grow and no
   periodic clicks should appear (aggregate-device drift compensation is
   enabled on the BlackHole sub-device with the mic as clock master).

## Known limitations (Phase 0, by design)

- Output device is found by name prefix `BlackHole`; the renamed signed
  fork arrives in Phase 1.
- TSE enrollment UI is not in the menu bar yet (the C ABI already accepts
  an enrollment WAV; the CLI is the enrollment path for now).
- The inference thread polls the input ring at 1 ms granularity instead
  of joining the device's `os_workgroup`; revisit if underruns appear.
- Input devices are assumed to support 48 kHz (`nsrt` is set at start;
  devices that reject it fail to start with a Core Audio error).

## Troubleshooting

- `no output device named BlackHole*`: BlackHole is not installed or
  `coreaudiod` has not picked it up (`sudo killall coreaudiod`).
- Engine starts but QuickTime hears nothing: check that the input device
  in the popover is the physical mic (not BlackHole), and that the mic
  permission was granted (System Settings → Privacy → Microphone).
- Crackling right after start: expected for < 1 s while the rings prime;
  persistent crackling with growing underruns is a bug — capture
  `noican_status_json` numbers and file an issue.
