# Hardware test: Setup — prerequisites, build, model weights

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

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
- A 48 kHz-capable microphone (the built-in microphone works) for the
  aggregate-path checks, and a microphone that cannot run at 48 kHz for
  the native-capture checks — any device from 8 to 192 kHz is captured
  natively through the split transport and resampled to 48 kHz inside
  it by the exact ratio (issue #7): a Bluetooth headset on a telephony
  profile (HFP/SCO at 8/16/24 kHz), or a 44.1 kHz-family device
  (44.1/22.05/11.025 kHz, e.g. a headset whose microphone only exposes
  CD-family rates). For telephony profiles expect narrow-band quality,
  and expect the headset's *playback* quality to drop while its
  microphone is in use (the whole headset falls into the phone profile)
  — both are properties of Bluetooth, noted in the UI, not defects. A
  44.1 kHz device is full-band; only the conversion is noted. Devices
  whose rate is unreadable or outside 8–192 kHz are refused.
- A **composite input/output microphone** for the aggregate-routing
  checks: a 48 kHz-capable USB microphone that also exposes output
  channels (a headphone jack — e.g. the Shure MV7+), or an audio
  interface with both inputs and outputs. Such a device appears in Audio
  MIDI Setup as *one* device with both an input and an output side; a
  headset that shows up as two separate devices (input-only plus
  output-only, as some Bluetooth headsets do) does not exercise this
  path.

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

Both variants are signed with `macos/Resources/Noican.entitlements`
(`com.apple.security.device.audio-input`), and the script fails if the
entitlement is missing from the finished signature. The Developer ID
variant additionally runs under the hardened runtime (`--options
runtime`), where `tccd` grants `kTCCServiceMicrophone` only to a
signature that carries that entitlement — a Developer ID bundle signed
without it (the state before 2026-09-09; see
[acceptance/2026-09-09-split-render-format.md](../acceptance/2026-09-09-split-render-format.md))
launches, logs a healthy engine, and never captures a sample: no prompt,
flat meters, "Noican does not react to my voice". The driver's
hardened-runtime signing is unaffected (no TCC-guarded capability).

Expected on a Developer ID build: the first start of the engine on a
Mac (or after a TCC reset) shows the **microphone permission prompt**.
No prompt *and* flat meters means the entitlement is missing; confirm
with `codesign --display --entitlements - dist/Noican.app` (the key must
be listed) and with the `tccd` log, which then contains `Prompting
policy for hardened runtime; service: kTCCServiceMicrophone requires
entitlement com.apple.security.device.audio-input but it is missing`:

```bash
/usr/bin/log show --last 5m --predicate 'process == "tccd"' \
  | grep -i noican | grep -E "requires entitlement|AUTHREQ_PROMPTING"
```

Read the `service:` field. A healthy Developer ID build logs one
`requires entitlement` line for **`kTCCServiceAppleEvents`** at launch
(`appleeventsd`'s launch handshake; the app sends no Apple Events and
nothing depends on it — recorded in
[acceptance/2026-09-09-developer-id-microphone.md](../acceptance/2026-09-09-developer-id-microphone.md))
and an `AUTHREQ_PROMPTING … service=kTCCServiceMicrophone` line when
the prompt is shown. Only a `requires entitlement` line for
**`kTCCServiceMicrophone`** is the regression.

TCC remembers the decision per bundle identifier, so a Mac that already
granted the microphone to an ad-hoc build will not prompt again for the
Developer ID build. To exercise the prompt, reset the record first:

```bash
pkill -x NoicanMenuBar
tccutil reset Microphone com.lightsound.noican
open dist/Noican.app
```

The build replaces the bundle on disk but does not touch a running
instance: after every rebuild quit the app (`pkill -x NoicanMenuBar`),
`open dist/Noican.app`, and confirm the PID in the Console lines has
changed before testing — a 2026-09-05 run reported a fix as ineffective
because the pre-fix process was still the one under test. If `swift
build` fails to find a type that exists in `macos/NoicanState`, remove
the stale SwiftPM caches (`rm -rf macos/.build macos/NoicanState/.build`)
and rebuild.

Validate it:

```bash
codesign --verify --deep --strict --verbose=2 dist/Noican.app
codesign --display --verbose=4 dist/Noican.app
codesign --display --entitlements - dist/Noican.app
```

The last command must list `com.apple.security.device.audio-input` with
value `true` on both variants; on the Developer ID variant
`--verbose=4` must also show `flags=0x10000(runtime)`.

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
