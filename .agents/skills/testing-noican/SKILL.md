---
name: testing-noican
description: How to end-to-end test the noican macOS noise-suppression product (Rust CLI fetch/process/eval + Swift menu-bar app) on a Mac VM
---

# Testing noican on a Mac VM

noican is a Rust workspace (5 crates) + SwiftPM menu-bar app. Most runtime paths are reachable headlessly; audio transports are not without real devices.

## Builds

- CLI: `cargo build --release --locked -p noican-cli` → `target/release/noican`. First build compiles heavy deps (ort downloads ONNX Runtime binaries at build time, deep_filter/tract) — needs network.
- App: `bash scripts/build-macos-app.sh` → `dist/Noican.app` (ad-hoc signed when `NOICAN_CODESIGN_IDENTITY` is unset; an audio-input entitlement is always required by the script). Launch with `open dist/Noican.app`; it is `LSUIElement` so it only adds a menu-bar status item (`mic.slash` when Off).

## CLI e2e without audio hardware

- `noican fetch <id>` — model ids live in `crates/noican-models/src/manifest.rs`. `ul-unas` is the smallest pinned-sha256 download (~790 KB, commit-pinned GitHub file). `fetch hush-48k` exercises `depends_on` (pulls the `hush` tar.gz into `models/hush/`).
- `fetch_model` re-verifies sha256 of present files: re-run `fetch` → "already present"; append a byte to a downloaded file → "checksum mismatch, re-downloading".
- Weights land in `./models/` relative to cwd (gitignored). The app uses `~/Library/Application Support/noican/models` instead.
- `noican process <wav>` default models = passthrough + every fetched stage → `out/<stem>/<model>.wav` (gitignored).
- `noican eval --target t.wav --interferer i.wav --segment-seconds N --sir 6` runs the speaker-suppression metrics harness.
- Generate speech WAVs with no audio device: `say -o /tmp/x.aiff "text"` then `afconvert -f WAVE -d LEI16@48000 /tmp/x.aiff /tmp/x.wav` (say supports `-v <voice>` for a second speaker).

## What is NOT verifiable on a VM with zero audio devices

`system_profiler SPAudioDataType` shows an empty Devices list on typical VMs. Then: engine start → "Select an input device"; the aggregate/split transports, `input_overruns` counter, monitor/preview, and DryWetMixer blend all need a live transport + the BlackHole/Noican virtual device (external/noican-driver submodule, driver install needs sudo) — treat as untestable, verify graceful degradation instead.

## GUI testing notes

- After `open`, find the status item by zooming the menu bar (`zoom` region like [820,0,910,22]) — its x position varies (~858 on 1024-wide tool space); clicks between icons silently do nothing.
- Expected no-device popover: header "Noican" + "No input device", segmented Off/Preview/On, "Mic: No Input devices", "Model & strength > <catalog name> · 100%", "Start at login", "Quit". Pressing "On" turns the status dot + icon red and shows "Select an input device" — this is the intended graceful failure, not a bug.
