# Hardware test: Output-underrun diagnostics (real-time budget)

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Output-underrun diagnostics (real-time budget)

A worker that misses its 10 ms block budget drains the output ring;
the virtual-microphone callback then zero-fills — audible as dropouts
and a lower average level in recordings — while Preview masks it
behind the monitor ring's re-priming cushion. The engine counts these
events on both transports: output callbacks that were fully starved —
zero real samples available for an entire I/O period (the start-up
ramp and benign partial shortfalls from 480-sample block quantization
are excluded by design) — and the worker's per-block processing
times (total blocks / blocks over 10 ms / maximum). Counters reset on
engine start and on every model switch, so readings are attributable
to the active model. The dated records
([2026-09-02](../acceptance/2026-09-02-underrun-baseline.md),
[2026-09-04](../acceptance/2026-09-04-underrun-reverify.md)) measured
DPDFNet2 and DeepFilterNet3 (the default) at zero underruns in both
runs, and DPDFNet8 with a one-shot burst in the first run. This
procedure re-checks them on each build.

The counters surface in the unified log — no popover UI by design
(they are a diagnosis tool, not a user control). In Console.app,
filter subsystem `com.lightsound.noican` (category
`engine-diagnostics`), or stream in a terminal:

```bash
log stream --predicate 'subsystem == "com.lightsound.noican"' --level info
```

One warning line appears for each 1 Hz health-poll tick in which the
underrun count grew, carrying the count, the active model id, and the
worker block statistics. In addition, one info line appears about a
second after every engine start — `Engine transport diagnostics:
worker realtime scheduling <bool>, Rosetta-translated process <bool>`
— reporting whether the inference worker's mach time-constraint
promotion succeeded and whether the process runs translated. Both must
read `true`/`false` respectively; a `false` realtime flag or a `true`
translation flag means the budget numbers measure scheduling or
translation overhead, not model cost (the first hardware run,
[2026-09-02](../acceptance/2026-09-02-underrun-baseline.md), showed
exactly that failure mode before the worker was promoted: chronic
41–49% budget misses on the heaviest model then registered and
one-shot 40 ms stalls even on light models).

1. Select On with a 48 kHz microphone (aggregate path) and record from
   the virtual device throughout. Confirm the transport line reads
   `worker realtime scheduling true` and
   `Rosetta-translated process false`.
2. For each of `DPDFNet2 48k HR` and `DeepFilterNet3 48k` (controls:
   zero underruns in both dated records), then `DPDFNet8 48k HR` and
   any model without a hardware baseline, such as `UL-UNAS 16k`,
   `Hush 16k`, and `Hush 48k` (suspects): select the model, speak
   continuously for at least 60 seconds, and note every diagnostic
   line (or its absence).
3. Pass criteria for the controls: **no underrun line at all** for
   DPDFNet2 and DeepFilterNet3. A nonzero count on a control is a
   regression and fails this check. Because DeepFilterNet3 is the
   default, the default model must pass.
4. For the suspects, record the counts verbatim (model, underruns,
   over-budget blocks / total blocks, max ms) into the result record.
   These numbers decide the countermeasure phase; do not tune anything
   from guesses.
5. Cross-check audibly: models that logged underruns must be the same
   ones whose recordings stutter; the recording of a model with zero
   underruns must be free of dropouts.
6. Repeat step 2 for one suspect on the split transport (a Bluetooth
   or 44.1 kHz microphone) to confirm the counter works there too. Do
   not compare split counts against aggregate counts numerically: the
   split ring is primed with a ~50 ms cushion the drift servo then
   maintains, so a single split underrun means the worker fell behind
   by the whole cushion — a far more severe event than one aggregate
   underrun, which only needs the shallow block-phase reservoir to run
   dry.

## Acceptance checklist (output-underrun diagnostics)

Run the Output-underrun diagnostics procedure above; the build passes
when:

0. **Worker is real-time**: the engine-start transport line reads
   `worker realtime scheduling true` and
   `Rosetta-translated process false`.
1. **Controls clean**: DPDFNet2 and DeepFilterNet3 (the default) log
   zero underruns over 60+ seconds of continuous speech on the
   aggregate path.
2. **Counts recorded**: DPDFNet8 and every other suspect have their
   underrun and block-time numbers recorded verbatim in the result
   record.
3. **Counts match ears**: models that log underruns are exactly the
   models whose virtual-microphone recordings stutter.
4. **Split transport covered**: at least one model's counters were
   exercised through a native-rate (Bluetooth or 44.1 kHz) microphone.
5. **No regression**: recordings, meters, preview, and model switching
   behave exactly as before on models with zero underruns.
