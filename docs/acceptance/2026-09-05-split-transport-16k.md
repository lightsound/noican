# Result record — Split transport on hardware (16 kHz HFP headset), 2026-09-05

First hardware run of the split native-capture transport, following the
"Non-48 kHz microphone (native-rate capture)" procedure of
[docs/macos-hardware-test.md](../macos-hardware-test.md) on the PR #24
build. This closes the split-transport gap left open by the
[2026-09-02 baseline](2026-09-02-underrun-baseline.md) and the
[2026-09-04 re-verification](2026-09-04-underrun-reverify.md)
(criterion 4 of the output-underrun checklist, "Split transport
covered"), which both recorded the owner's headset as refused.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air 15-inch, 2023 — Apple M2, 24 GB (same machine as the prior records) |
| macOS version | macOS Tahoe 26 |
| App commit | `cursor/split-rational-resample-0035` at `5294b90` (PR #24) |
| Microphone | HUAWEI FreeClip — Audio MIDI Setup shows it as two devices: **"HUAWEI FreeClip 1"** (input 1 / output 0, format **16,000 Hz, 1 ch, 16-bit integer, fixed**) and "HUAWEI FreeClip 2" (input 0 / output 2, 44.1 kHz selectable — the playback side, not used by the engine) |
| Transport | Split (input-only AUHAL at 16 kHz → 3/1 polyphase → 48 kHz output-only AUHAL), strength as selected by the owner |
| Model | Not recorded in the owner's report; the app's default selection is FastEnhancer-B |

### What the rate reading corrected

The 2026-09-02 and 2026-09-04 records describe the headset as capturing
"only at 44.1 kHz-family rates". Reading the device in Audio MIDI Setup
shows that the 44.1 kHz device is the **playback** half; the microphone
half is a fixed 16 kHz HFP stream — an integer-factor (3/1) rate the
split transport accepted since issue #7. The earlier refusal is
therefore not reproduced on this build and its cause was not
re-investigated (PR #24 accepts both rate families, so a 44.1 kHz
nominal at start time would also have been converted). Consequently
**this run exercises the integer-factor 3/1 path**, not the rational
160/147 path PR #24 adds; the rational path's evidence remains the unit
tests and the closed-loop simulation in `noican-core` until a
44.1 kHz-only *input* device is available.

## Measured (verbatim log, `log stream --predicate 'subsystem == "com.lightsound.noican"' --level info`)

```
2026-09-05 08:49:23.238592+0900 ... Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
2026-09-05 08:51:37.146640+0900 ... Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
```

Two engine sessions (the second after the microphone round-trip below),
both promoted to real-time scheduling. **No underrun diagnostic line was
emitted** during a 60 s recording through the headset — zero output
underruns on the split transport.

## Owner observations

- **Recording quality**: judged acceptable; the owner notes the
  headset's own microphone quality is modest, which limits how much a
  listening test can tell (telephony-bandwidth capture is the expected
  ceiling on HFP).
- **Microphone round-trip (built-in ↔ headset) while speaking**: one
  brief interruption at the moment of each switch, then clean. This is
  the transport rebuild (aggregate ↔ split): the procedure's step 1
  ("Microphone switching") states a short gap is inherent; a dead
  stream or stale aggregate would be the failure, and neither occurred.
- **Selection notice** shown under the microphone list: "HUAWEI
  FreeClip captures at 16 kHz (Bluetooth phone profile): audio is
  narrow-band — resampling can't restore full quality — and headset
  playback quality also drops while the microphone is in use." This is
  the intended telephony-profile wording for a 16 kHz HFP device
  (`CaptureSupport.isTelephonyRate`).
- **30-minute drift/endurance run**: not performed in this session
  (owner's choice); remains open below.

## Acceptance outcome

Against the "Acceptance checklist (native-rate capture, issue #7)":

| # | Criterion | Outcome |
|---|---|---|
| 1 | Running on a native rate | **Pass** (16 kHz HFP; two sessions reached Running) |
| 2 | 48 kHz output, intelligible | **Pass** (recording judged acceptable; telephony bandwidth as expected) |
| 3 | No drift artifacts over 30 min | **Not run** (deferred by the owner) |
| 4 | Aggregate path unchanged | **Pass** (built-in microphone sessions in the round-trip) |
| 5 | Strength alignment (single voice at 50%) | **Not reported** |
| 6 | Rate-change recovery (A2DP ↔ HFP) | **Not reported** |
| 7 | UI truthfulness (rate label, notice kind) | **Pass** ("16 kHz", telephony notice) |
| 8 | Real-time constraints (worker promoted; callbacks unchanged) | **Pass** (`worker realtime scheduling true`, both sessions) |
| 9 | Split-path underruns: zero over 60+ s | **Pass** (no underrun line emitted) |

Against the "Acceptance checklist (output-underrun diagnostics)":

| # | Criterion | Outcome |
|---|---|---|
| 4 | Split transport covered | **Pass** — closes the item left Unverified on 2026-09-02 and 2026-09-04 |

## Open items

- Criterion 3 (30-minute endurance on the split transport) and
  criteria 5–6 are not yet evidenced on hardware; run them when
  convenient and append a record.
- The rational-ratio path (44.1 kHz family) has no hardware run; it is
  covered by `noican-core` unit tests (93–97 dB SNR, closed-loop servo
  at 160/147) and will get a record when a 44.1 kHz-only input device
  is available.
