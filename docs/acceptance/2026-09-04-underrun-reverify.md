# Result record — Output-underrun diagnostics, re-verification, 2026-09-04

Re-run of the "Output-underrun diagnostics (real-time budget)"
procedure of [docs/macos-hardware-test.md](../macos-hardware-test.md)
on the Phase B build (PR #23, worker promoted to mach time-constraint
scheduling), against the
[2026-09-02 baseline](2026-09-02-underrun-baseline.md) taken on the
same hardware.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air 15-inch, 2023 — Apple M2, 24 GB |
| macOS version | macOS Tahoe 26 |
| App commit | `cursor/fel-realtime-budget-3417` at `b79dbbc` (PR #23) |
| Transport | Aggregate (48 kHz built-in microphone), strength 100% |

## Measured diagnostics (verbatim log values)

The one-time transport line read `worker realtime scheduling true,
Rosetta-translated process false` in both engine sessions — the
promotion works on device.

Underrun lines (one per 1 Hz growth tick; counters reset on every
model switch and engine start):

| Time (JST) | Model | Underruns | Over-budget / total blocks | Max |
|---|---|---|---|---|
| 20:17:13 | fastenhancer-l | 5 | 29/68 (43%) | 19.6 ms |
| 20:18:08 | fastenhancer-l (after engine restart) | 5 | 31/71 (44%) | 21.3 ms |

`fastenhancer-b`, `dpdfnet2`, `dpdfnet8`, and `deepfilternet3`
produced **no diagnostic line at all** over their 60 s runs (zero
underruns).

## Acceptance outcome

Against the "Acceptance checklist (output-underrun diagnostics)" of
[docs/macos-hardware-test.md](../macos-hardware-test.md):

| # | Criterion | Outcome |
|---|---|---|
| 0 | Worker is real-time | **Pass** (`true`/`false` line, both sessions) |
| 1 | No false positives on light models | **Pass** (zero lines over 60+ s) |
| 2 | Suspect counts recorded verbatim | **Pass** (table above) |
| 3 | Counts match ears | **Pass** (only FE-L stutters, only at selection) |
| 4 | Split transport covered | **Unverified** — see below |
| 5 | No regression on clean models | **Pass** |

**Criterion 4 remains unverified in both this run and the baseline.**
`split_processing_loop` received the identical real-time promotion, but
no compatible microphone was available: the owner's Bluetooth headset
captures only at 44.1 kHz-family rates, which the app refuses (no
integer factor to 48 kHz). The split transport therefore carries **no
hardware evidence for this fix**; every claim in this record is scoped
to the aggregate transport. Deferred until a 48 kHz-capable Bluetooth
(or other native-rate) microphone is available.

*Forward pointer (2026-09-04):* PR #24 lifts this blocker — the split
transport now converts any 8–192 kHz rate by the exact ratio, and the
headset's microphone side was since read as 16 kHz HFP (its 44.1 kHz
device is the playback side). The split-transport re-run is pending and
will be recorded separately.

## Reading against the baseline

- **The chronic budget misses are gone.** In the baseline,
  FastEnhancer-L kept emitting growth ticks throughout its run
  (41–49% of blocks over budget at 392–506 total blocks, max
  80.5 ms). Now each 60 s run emits exactly one tick, whose total
  block count (68 and 71 blocks ≈ 0.7 s of audio) places every
  underrun inside the first second after the model goes live; the
  counter never grows again for the rest of the run, and the maximum
  block time fell from 80.5 ms to ~20 ms.
- **The light-model false-alarm bursts are gone.** The baseline's
  one-shot bursts on FastEnhancer-B (10 underruns, max 40.7 ms) and
  DPDFNet8 (2 underruns, max 46.5 ms) did not reproduce: all four
  non-FE-L models ran their full 60 s without a single underrun,
  meeting the acceptance criterion for the light controls. This
  confirms the baseline bursts were scheduling stalls, not model
  cost.
- **What remains on FastEnhancer-L is a switch-time warm-up burst**:
  the first blocks through a freshly created ONNX Runtime session pay
  one-time lazy initialization (kernel selection, buffer allocation),
  and FE-L's steady-state cost — p50 4.16 ms on a slower x86-64 host
  (`noican-models/examples/block_bench.rs`) — leaves the least
  headroom of any model to absorb it. Audibly this is at most a brief
  stutter in the first second after selecting the model; steady-state
  operation is clean.

Conclusion: on the aggregate transport, the real-time promotion
resolves the recording dropouts and level drop attributed to
output-ring underruns. The residual FE-L warm-up burst is confined to
the moment of model selection; if it proves audible enough to matter,
the candidate follow-up is to warm the session with a few dummy blocks
on the loader (control) thread before the lock-free swap — out of
scope for PR #23. The split transport's promotion is code-identical
but unexercised on hardware (criterion 4 above).
