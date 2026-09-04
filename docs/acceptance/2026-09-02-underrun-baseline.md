# Result record — Output-underrun diagnostics, first hardware run, 2026-09-02

First on-device run of the "Output-underrun diagnostics (real-time
budget)" procedure of
[docs/macos-hardware-test.md](../macos-hardware-test.md), on the Phase A
build (PR #22, merged to main as `0b9c2a3`). These numbers are the
baseline that the worker real-time-scheduling fix must improve on.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air 15-inch, 2023 — Apple M2, 24 GB |
| macOS version | macOS Tahoe 26 |
| App commit | main `0b9c2a3` (PR #21 + #22 merged) |
| Transport | Aggregate (48 kHz built-in microphone), strength 100% |

## Measured diagnostics (verbatim log values)

One row per unified-log line (each line is one 1 Hz growth tick;
counters reset on every model switch):

| Time (JST) | Model | Underruns | Over-budget / total blocks | Max |
|---|---|---|---|---|
| 17:20:14 | fastenhancer-b | 10 | 2/714 (0.3%) | 40.7 ms |
| 17:23:34 | dpdfnet8 | 2 | 10/300 (3.3%) | 46.5 ms |
| 17:26:27 | fastenhancer-l | 7 | 39/80 (49%) | 59.1 ms |
| 17:26:30 | fastenhancer-l | 13 | 168/392 (43%) | 80.5 ms |
| 17:26:31 | fastenhancer-l | 22 | 218/506 (43%) | 80.5 ms |
| 17:28:03 | fastenhancer-l (reselected) | 2 | 43/102 (42%) | 31.1 ms |
| 17:28:09 | fastenhancer-l | 4 | 42/99 (42%) | 21.7 ms |
| 17:28:13 | fastenhancer-l | 6 | 205/501 (41%) | 49.7 ms |

`dpdfnet2` and `deepfilternet3` produced no diagnostic line over their
60 s runs (zero underruns).

**Unverified: split transport (acceptance criterion 4).** The
split-transport repeat could not be exercised: the available Bluetooth
headset is refused by the app (44.1 kHz-family capture rates only, no
integer factor to 48 kHz). All numbers in this record are from the
aggregate transport; the split transport has no baseline.

*Forward pointer (2026-09-04):* PR #24 lifts this blocker — the split
transport now converts any 8–192 kHz rate by the exact ratio. The
split-transport re-run is pending and will be recorded separately.

## Reading

- **FastEnhancer-L misses the budget chronically**: 41–49% of blocks
  over 10 ms across two independent selections, max 80.5 ms — the
  recordings' stutter and level drop are fully explained.
- **The light models are not clean either**: FastEnhancer-B logged one
  10-underrun burst (2/714 blocks over, max 40.7 ms) and DPDFNet8 an
  early 2-underrun burst, then both ran clean for the rest of their
  60 s. One-shot multi-tens-of-ms stalls on models this light point at
  scheduling, not model cost.
- **The same models measure far inside the budget when scheduled**: on
  a modest 4-core x86-64 host (`noican-models/examples/block_bench.rs`,
  6000 blocks), FastEnhancer-L reads p50 4.16 ms / p95 6.23 ms /
  max 12.6 ms with zero to 0.1% over budget, and FastEnhancer-B
  p50 0.35 ms. An M2 performance core is faster than that host, so the
  on-device numbers cannot be explained by model cost alone.

Conclusion: the inference worker joined the audio `os_workgroup` but was
never promoted to mach time-constraint (real-time) scheduling, leaving
it at default priority — schedulable on efficiency cores and
preemptible for tens of milliseconds, which matches both the chronic
FastEnhancer-L misses and the one-shot bursts on light models. The fix
(promoting the worker before the workgroup join, surfaced as
`worker realtime scheduling true` in the transport diagnostics line)
must be re-verified on this hardware with the same procedure.

Re-verified on the same hardware on 2026-09-04 — see the
[re-verification record](2026-09-04-underrun-reverify.md): the chronic
misses and the light-model bursts are gone; only a sub-second warm-up
burst remains on FastEnhancer-L at model selection.
