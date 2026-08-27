# Acceptance record — Noican driver (Driver check), 2026-08-27

Result record for the Driver check and coexistence acceptance of the
Noican virtual driver (PR #11), per the "Result record" section of
[docs/macos-hardware-test.md](../macos-hardware-test.md).

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air 15-inch, 2023 — Apple M2, 24 GB |
| macOS version | macOS Tahoe 26.6.2 (build 25G83) |
| Xcode version | Xcode 26.6 (build 17F113) |
| App / driver commit | PR #11 head `6b8be01` (merged to main as `6e728a1`) |
| BlackHole submodule | v0.7.1 (`e2b22aa`) |
| Driver signature | Developer ID Application (Team `6R926386F6`); see below |
| Virtual device UID | `com.lightsound.noican.2ch_UID` |
| Physical microphone | HUAWEI FreeClip (Bluetooth) |
| Sample rate | 48 kHz (2 ch, 32-bit float) |

### Signature (`codesign --display --verbose=4 dist/Noican.driver`, excerpt)

```text
Identifier=com.lightsound.noican.driver
Format=bundle with Mach-O universal (x86_64 arm64)
CodeDirectory v=20500 size=392 flags=0x10000(runtime) hashes=5+3 location=embedded
CDHash=ac28eae23ac4ae72b4c4121d1f9aefc9c39fcfe2
Authority=Developer ID Application: Qin, G.K. (6R926386F6)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Aug 27, 2026 at 13:51:50
TeamIdentifier=6R926386F6
Runtime Version=26.5.0
Sealed Resources version=2 rules=13 files=4
```

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
