# Noican

A personal, fully on-device noise-cancelling virtual microphone for macOS, in the spirit of [JoyCast](https://joycast.ai/).

The physical microphone signal is captured, cleaned in real time (noise suppression and, eventually, background-speaker suppression), and re-exposed to the system as a virtual input device that any app (Zoom, Meet, Teams, OBS, Discord, ...) can select as its microphone. Everything runs locally on Apple Silicon; no audio ever leaves the machine.

## Status

Phase 0 code-complete: switchable audio engine (Rust) with a CLI
batch-comparison mode, plus the real-time macOS pipeline (AUHAL on a private
Aggregate Device, routed into a loopback device) and a SwiftUI menu-bar app.
Phase 1 driver work has started: the Noican-branded virtual device
([docs/driver.md](docs/driver.md)) builds from this repository and takes
priority over stock BlackHole 2ch when both are installed. Hardware
acceptance (audio output, TCC, model switching, driver loading on a real
Mac) is tracked in
[docs/macos-hardware-test.md](docs/macos-hardware-test.md).

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

### CLI input formats

`process` accepts WAV, AIFF/AIFC, CAF, and M4A (AAC or Apple Lossless)
inputs; outputs are always mono 48 kHz 16-bit WAV. Compressed AIFC
variants outside the PCM/µ-law/A-law family (e.g. IMA4) and other exotic
encodings are not decodable here — convert them once with macOS's built-in
`afconvert`:

```sh
afconvert -f WAVE -d LEI16@48000 input.aifc output.wav
```

### macOS menu bar app

```sh
# Requires: Apple Silicon, Rust target aarch64-apple-darwin, Swift 6.1+,
# and a loopback driver: the Noican driver (below) or stock BlackHole 2ch.
bash scripts/build-macos-app.sh   # produces dist/Noican.app
```

See [docs/macos-hardware-test.md](docs/macos-hardware-test.md) for the
build details and the on-hardware acceptance checklist.

### Noican virtual driver (BlackHole fork)

The Noican-branded loopback device ("Noican Microphone", 2 ch / 48 kHz) is
built from the `external/blackhole` submodule without patching upstream —
all customization is injected at build time ([docs/driver.md](docs/driver.md)):

```sh
git submodule update --init                       # once, after cloning

bash scripts/build-driver.sh                      # ad-hoc (compile check)
NOICAN_CODESIGN_IDENTITY="Developer ID Application: ... (TEAMID)" \
  bash scripts/build-driver.sh                    # installable build

bash scripts/install-driver.sh                    # sudo; restarts coreaudiod
bash scripts/uninstall-driver.sh                  # sudo; complete removal
```

macOS 15+ `coreaudiod` only loads Developer-ID-signed drivers. The driver
is GPL-3.0 (see `LICENSE.driver`) and stays a separate program — its
sources are never linked into the app.

### Quality gates

All enforced in CI: `cargo fmt --check`; `cargo clippy` with the exhaustive
rustc lint set plus `pedantic`/`nursery`/`cargo` as errors (opt-outs only
via `#[expect(..., reason = "...")]`); rustdoc lints; `cargo test`;
`cargo deny check`; `cargo machete`. On the macOS runner additionally:
SwiftLint in strict mode and `swift build -Xswiftc -warnings-as-errors`.

## Workspace layout

- `crates/noican-core` — engine-agnostic core: `Stage`/`FrameProcessor`
  traits at a fixed 48 kHz engine rate, streaming polyphase resampling,
  frame adaptation, and the lock-free `SwitchingEngine` for click-free
  runtime model switching.
- `crates/noican-models` — model registry, SHA-256-verified weight
  fetching, and stage implementations (ONNX Runtime + tract backends).
- `crates/noican-cli` — `noican` binary: `models` / `fetch` / `process`
  (batch audio comparison under strictly identical conditions).
- `crates/noican-coreaudio` — AUHAL real-time transport on a private
  Aggregate Device (macOS; portable stub elsewhere).
- `crates/noican-ffi` — C ABI consumed by the Swift control plane
  (engine lifecycle + registry-driven model catalog).
- `macos/` — SwiftPM package for the `MenuBarExtra` control-plane app.
- `external/blackhole` — upstream BlackHole submodule (GPL-3.0, pinned to
  a release tag); built into the separate `Noican.driver` by
  `scripts/build-driver.sh` (docs/driver.md).

## Scope

- macOS only (Apple Silicon first), personal use only.
- No distribution, licensing, or multi-user concerns; a paid Apple Developer Program membership is available for Developer ID signing.
- Target: ~20–30 ms end-to-end latency, < 100 MB memory, 48 kHz native audio path.
