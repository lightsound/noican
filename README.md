# noican

A personal, fully on-device noise-cancelling virtual microphone for macOS, in the spirit of [JoyCast](https://joycast.ai/).

The physical microphone signal is captured, cleaned in real time (noise suppression and, eventually, background-speaker suppression), and re-exposed to the system as a virtual input device that any app (Zoom, Meet, Teams, OBS, Discord, ...) can select as its microphone. Everything runs locally on Apple Silicon; no audio ever leaves the machine.

## Status

Phase 0: the engine runs, eleven models are selectable at run time, the offline comparison tool works, and the macOS audio path and menu bar app are written and build. **Nothing has been run on a Mac** — CI has no audio devices and no way to hear the result, so what it proves is that the code compiles, links, and signs.

- [docs/tech-research.md](docs/tech-research.md) — the single source of truth: candidate evaluation for every layer, the chosen stack, the roadmap, and what measurement has and has not confirmed.
- [docs/models.md](docs/models.md) — the model catalog, how to fetch weights, and how to run a comparison.
- [docs/manual-testing.md](docs/manual-testing.md) — the hardware test procedure, and the list of things most likely to be wrong.

## Design in one paragraph

Every processing step is a `Stage`: a native sample rate, a block size, an algorithmic delay, and a `process` call. `StageRunner` adapts any stage to the 48 kHz host path, so a 16 kHz model with a 160-sample block and a 48 kHz model with a 512-sample block are equally drop-in. Models are therefore interchangeable at run time rather than chosen up front, adding one costs a single trait implementation, and the offline comparison runs the same engine as the live path — so what you compare is what you will hear.

## Layout

| Path | What it is |
|---|---|
| `crates/noican-core` | The `Stage` abstraction and the real-time-safe DSP under it: fixed-capacity queues, polyphase rational resampling, streaming STFT/ISTFT |
| `crates/noican-models` | ONNX-backed stages, the model catalog, and weight acquisition |
| `crates/noican-engine` | The inference thread, the lock-free hand-off to the audio callback, and click-free model switching. Platform-independent, so the switching logic is testable without a Mac |
| `crates/noican-cli` | `noican`: fetch and verify weights, process WAV files, measure model delay |
| `crates/noican-macos` | Core Audio: device enumeration, the private aggregate device, and the I/O proc |
| `crates/noican-ffi` | The C ABI the Swift app calls |
| `apps/NoicanMenuBar` | SwiftUI `MenuBarExtra`: on/off, device pickers, model picker, meters |

## Getting started

```bash
cargo run -p noican-cli -- list                 # what models exist
cargo run -p noican-cli -- fetch                # download all weights (~62 MB)
cargo run --release -p noican-cli -- process recording.wav
```

That writes `out/recording/`: the unprocessed reference, one WAV per model, and a manifest recording each model's measured delay, speed, and level. See [docs/models.md](docs/models.md).

## Scope

- macOS only (Apple Silicon first), personal use first; a paid Apple Developer Program membership is available for Developer ID signing.
- Target: ~20–30 ms end-to-end latency, < 100 MB memory, 48 kHz native audio path.
- Third-party licences and attributions are collected in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). The GPL-3.0 virtual-audio driver is a separate program loaded by `coreaudiod` and is never linked into this workspace.

## Development

Every check runs at error level; see `docs/tech-research.md` §12 for the policy and its two deliberate exceptions.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny --all-features check     # cargo install cargo-deny
cargo machete                       # cargo install cargo-machete
```
