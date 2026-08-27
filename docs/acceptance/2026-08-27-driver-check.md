# Acceptance record — Noican driver (Driver check), 2026-08-27

Result record for the Driver check and coexistence acceptance of the
Noican virtual driver (PR #11), per the "Result record" section of
[docs/macos-hardware-test.md](../macos-hardware-test.md). Items marked
`TODO(owner)` are known only to the person who ran the test.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air (Apple Silicon) — TODO(owner): exact model/chip |
| macOS version | macOS 26 — TODO(owner): exact build (`sw_vers`) |
| Xcode version | TODO(owner): `xcodebuild -version` |
| App / driver commit | PR #11 head `6b8be01` (merged to main as `6e728a1`) |
| BlackHole submodule | v0.7.1 (`e2b22aa`) |
| Driver signature | Developer ID Application — TODO(owner): paste `codesign --display --verbose=4 dist/Noican.driver` output |
| Virtual device UID | `com.lightsound.noican.2ch_UID` |
| Physical microphone | TODO(owner): device used with the app during (a)/(b) |
| Sample rate | 48 kHz (2 ch, 32-bit float) |

## Results

| Check | Result |
|---|---|
| Developer ID build (`scripts/build-driver.sh` with `NOICAN_CODESIGN_IDENTITY`) + `codesign --verify --deep --strict` | Pass |
| Install (`scripts/install-driver.sh`, coreaudiod restart) | Pass |
| Audio MIDI Setup: "Noican Microphone", 2 ch, 48 kHz, manufacturer `lightsound`, input + output streams (`system_profiler` output retained in PR #11 conversation) | Pass |
| Raw loopback (output → input, no Noican app) | Pass |
| (a) Noican driver only: app reaches Running; processed voice on Noican Microphone | Pass |
| (b) Coexistence with stock BlackHole 2ch: processed voice on Noican Microphone, silence on BlackHole 2ch (Noican preferred) | Pass |
| Meeting app lists "Noican Microphone"; recording carries processed audio | Pass |
| Preview refusal with default output = Noican loopback | Pass |
| Uninstall (`scripts/uninstall-driver.sh`): bundle gone, no device left in Audio MIDI Setup | Pass |

## Observations

- QuickTime's monitor slider, combined with the default output set to the
  loopback itself, forms a digital feedback loop and the recording layers
  on itself. Expected loopback behavior, not a driver defect; slider at
  zero produces a clean single-layer recording.
- With JoyCast.driver still installed, coreaudiod's bundle scan logged
  duplicate CFPlugIn factory-UUID warnings (all unpatched BlackHole forks
  share upstream's hardcoded UUID). Harmless — every device loaded and ran
  correctly — but addressed by the build-time factory-UUID rewrite added
  after this run (docs/driver.md, "CFPlugIn factory UUID").

## Out of scope for this run

Callback audit, clock-drift endurance, and the full Phase 0 hybrid
checklist are separate acceptance items of docs/macos-hardware-test.md and
were not part of this driver-focused run.
