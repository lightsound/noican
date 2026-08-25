# noican

A personal, fully on-device noise-cancelling virtual microphone for macOS, in the spirit of [JoyCast](https://joycast.ai/).

The physical microphone signal is captured, cleaned in real time (noise suppression and, eventually, background-speaker suppression), and re-exposed to the system as a virtual input device that any app (Zoom, Meet, Teams, OBS, Discord, ...) can select as its microphone. Everything runs locally on Apple Silicon; no audio ever leaves the machine.

## Status

Phase 0 in progress: switchable audio engine (Rust) with a CLI batch-comparison
mode; the real-time macOS pipeline and menu-bar UI are under construction.

- [docs/tech-research.md](docs/tech-research.md) — consolidated technology research: candidate evaluation for every layer, final recommended stack, roadmap, and open questions.
- [docs/models.md](docs/models.md) — supported models, weight download, and verification status.

## Building

Rust 1.98+ (pinned via `rust-toolchain.toml`).

```sh
cargo build --release

# Download model weights (see docs/models.md)
cargo run -p noican-cli --release -- fetch

# Compare all models on a recording (outputs under out/<stem>/<model>.wav)
cargo run -p noican-cli --release -- process my_recording.wav
```

Quality gates (all enforced in CI): `cargo fmt --check`,
`cargo clippy` with `pedantic`/`nursery` as errors, `cargo test`,
`cargo deny check`, `cargo machete`.

## Workspace layout

- `crates/noican-core` — engine-agnostic core: `Stage`/`FrameProcessor`
  traits at a fixed 48 kHz engine rate, streaming polyphase resampling,
  frame adaptation. Every model is one trait implementation; models are
  switchable at runtime.
- `crates/noican-models` — model registry, SHA-256-verified weight
  fetching, and stage implementations (ONNX Runtime + tract backends).
- `crates/noican-cli` — `noican` binary: `models` / `fetch` / `process`
  (batch WAV comparison under strictly identical conditions).

## Scope

- macOS only (Apple Silicon first), personal use only.
- No distribution, licensing, or multi-user concerns; a paid Apple Developer Program membership is available for Developer ID signing.
- Target: ~20–30 ms end-to-end latency, < 100 MB memory, 48 kHz native audio path.
