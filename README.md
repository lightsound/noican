# noican

A personal, fully on-device noise-cancelling virtual microphone for macOS, in the spirit of [JoyCast](https://joycast.ai/).

The physical microphone signal is captured, cleaned in real time (noise suppression and, eventually, background-speaker suppression), and re-exposed to the system as a virtual input device that any app (Zoom, Meet, Teams, OBS, Discord, ...) can select as its microphone. Everything runs locally on Apple Silicon; no audio ever leaves the machine.

## Status

Phase 0 implementation:

- [docs/tech-research.md](docs/tech-research.md) — consolidated technology research: candidate evaluation for every layer, final recommended stack, roadmap, and open questions.
- Rust engine with a common fixed-frame model trait, sample-rate adaptation, lock-free replacement, and bounded mute transitions.
- Real inference backends for FastEnhancer T/B/S, DPDFNet2/8 HR, DeepFilterNet3, UL-UNAS, and Hush, plus an explicitly selected experimental TSE Conv-TasNet backend.
- Reproducible multi-model WAV comparison CLI and checksum-verified model cache.
- AUHAL transport, private Aggregate Device control, and a minimal SwiftUI `MenuBarExtra`.
- [docs/model-assets.md](docs/model-assets.md) — model provenance, downloads, CLI comparison, and the current TSE access blocker.
- [docs/macos-hardware-test.md](docs/macos-hardware-test.md) — macOS build and hardware acceptance procedure.

## CLI

```bash
cargo run -- models list
cargo run -- process fixtures/sample-noisy.wav \
  --model fastenhancer-t,fastenhancer-b,fastenhancer-s,\
dpdfnet2-48khz-hr,dpdfnet8-48khz-hr,deepfilternet3,ul-unas,hush
```

Outputs and a deterministic `comparison.json` are written below `output/`. With no model flags, the CLI uses the eight Phase 0 variants and excludes gated TSE assets.

## macOS app

On an Apple Silicon Mac with Xcode:

```bash
bash scripts/build-macos-app.sh
open dist/noican.app
```

Install stock BlackHole 2ch for Phase 0 or the separately signed noican driver fork first. The app uses AUHAL directly; it does not use `AVAudioEngine`.

## Scope

- macOS only (Apple Silicon first), fully on-device.
- Commercial distribution is an architectural goal, not a cleared legal conclusion. The GPL-3.0 BlackHole-derived driver stays separate; combined distribution requires legal review or a commercial BlackHole license.
- The TSE repository currently requires authentication, and its reported license conflicts with DEMAND provenance. Do not redistribute it without review.
- Target: ~20–30 ms end-to-end latency, < 100 MB memory, 48 kHz native audio path.
