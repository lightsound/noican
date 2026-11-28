# 0002. Engine diagnostics and observability

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## diagnostic-counters

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::output_underruns`.

Diagnostic: output callbacks that were fully starved — zero
real samples popped — after the ring first produced audio (the
unprimed ramp-up on the aggregate path is start-up latency, not
underrun, and partial shortfalls are benign block-quantization
jitter). A count that keeps growing means the inference worker
misses its 10 ms block budget — audible as dropouts in
recordings from the virtual microphone. Resettable via
[`Runtime::reset_debug_stats`] for per-model attribution.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::input_overruns`.

Diagnostic: input samples the capture ring dropped because it
was full — the inference worker (or the capture side) fell
behind by more than the ring's capacity. Audible as clipped
input reaching the models; resettable via
[`Runtime::reset_debug_stats`] for per-model attribution.

Source: `crates/noican-coreaudio/src/macos/mod.rs`, `Runtime::reset_debug_stats`.

Zeroes the diagnostic counters (underruns, input overruns and
worker block statistics) so a model switch can be measured in
isolation. Plain relaxed stores — safe while the callbacks and
the worker run.

Source: `crates/noican-coreaudio/src/pump.rs`, `Pump::run_block`.

The block duration is saturated into a `u64` of nanoseconds (585
years — unreachable in practice, but the stats must never panic).

## diagnostics-counter-baseline

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.lastUnderrunCount`.

Last underrun count already reported. The baseline resets with
the engine counters: on engine start (via `reset()`) and on the
post-switch stats reset (a counter that moved backwards only
rebases silently).

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.lastVirtualOutputLevel`.

Last virtual-output level reading written to the log. Deliberately
*not* reset with the engine counters: the Noican Microphone slider
belongs to the device, not to a transport, so a restart with the
slider still down must not log a fresh "detection" of a slider
that never moved — the log is meant as evidence of how often and
how far it actually moves.

## engine-diagnostics

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::frames_processed`.

Heartbeat: total input frames delivered since the engine started.

Reads 0 while stopped. A value that stops advancing while
`NoicanHealth::running` is set means the device stopped calling
back (unplugged microphone, coreaudiod restart, post-sleep stall); the
UI polls this once per second.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::output_underruns`.

Diagnostic: output callbacks that delivered no real audio at all —
the output ring was dry for the entire I/O period.

Counted only after the ring first carried real audio (the start-up
ramp is latency, not underrun), and partial zero-fills are not
counted (benign block-quantization jitter — see the render
callbacks). Reads 0 while stopped. A growing
count means the inference worker misses its 10 ms block budget for
the active model — audible as dropouts in recordings from the
virtual microphone, while the preview monitor masks it behind its
re-priming cushion. Counted on both transports (aggregate and
split).

Cumulative since start or the last [`noican_engine_reset_debug_stats`];
reset after a model switch to attribute underruns to one model.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::input_overruns`.

Diagnostic: input samples the capture ring dropped because it was
full.

A full ring means the inference worker (or the capture side) fell
behind by more than the whole ring — audible as clipped input
reaching the models. Counted on both transports (aggregate and
split), cumulative since start or the last
[`noican_engine_reset_debug_stats`]; reads 0 while stopped.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::worker_blocks`.

Diagnostic: engine blocks the inference worker has processed since
start or the last [`noican_engine_reset_debug_stats`] (the
denominator for the over-budget count). Reads 0 while stopped.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::worker_blocks_over_budget`.

Diagnostic: worker blocks whose processing exceeded the 10 ms budget.

Counted since start or the last [`noican_engine_reset_debug_stats`].
Reads 0 while stopped. Blocks over budget are what drain the
output ring into underrun.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanHealth::worker_realtime`.

Diagnostic: set when the inference worker's mach time-constraint
(real-time) promotion succeeded at engine start, clear otherwise
(including while stopped and for a null handle).

Clear while running means the promotion failed and the worker runs at
default priority — budget misses in that state may be scheduling,
not model cost, and hardware underrun numbers should be read with
that in mind. The value is the start-time promotion result, not
live membership: the kernel may later demote a persistently
overrunning thread without this flag changing.

Source: `crates/noican-ffi/src/lib.rs`, `noican_engine_reset_debug_stats`.

Zeroes the diagnostic counters of the running engine.

Resets the output underruns, input overruns and the worker block
statistics so a freshly selected model can be measured in isolation. A no-op while
stopped or for a null handle. Plain atomic stores on the Rust side —
safe while audio runs.

# Safety

`handle` must be null or a live engine handle.

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.setModel`.

Zero the underrun/block-time diagnostics on every successful
switch, so the health poll attributes what follows to the
newly published model (see EngineDiagnostics).

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `RustEngine.resetDebugStats`.

Zeroes the diagnostic counters so a freshly selected model can
be measured in isolation. A no-op while stopped.

## engine-diagnostics-log-only

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics`.

Reports the engine's real-time budget diagnostics (output-ring
underruns plus the worker's block-time statistics) to the unified
log. Underruns are a hardware-diagnosis tool, not a user-facing
control, so nothing renders in the popover by design: one warning
line per growth tick — sampled by the 1 Hz health poll — keeps the
UI quiet while the counter, the active model, and the block
statistics stay retrievable from Console.app (subsystem
`com.lightsound.noican`, category `engine-diagnostics`).

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.sample`.

Samples the engine's counters and logs one warning line when the
underrun count grew since the previous sample, carrying the
active model id and the worker block statistics — enough to
attribute dropouts to a model on hardware without any UI.
`splitTransport` says which transport shape the session runs
(the control plane knows it from the start path it took) and
only selects the prefix of the one-time routing line.

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.sample`.

Where the engine output lands, as AUHAL reports it: inside
the aggregate (channel map) on the aggregate path, or the
virtual output's channel count and the render format sized
from it on the split path. Neither is visible otherwise,
and this is the line to read when the virtual microphone
records silence. The prefixes differ so the acceptance
procedures can pin each transport's line by name.

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift`, `EngineDiagnostics.sample`.

Input-side counterpart: capture-ring overruns drop real
audio before it ever reaches the engine — underruns alone
cannot show it, and `frames_processed` still advances.

## engine-diagnostics-logging

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.diagnostics`.

Real-time budget diagnostics (output underruns, worker block
times), sampled by the 1 Hz health poll and reported to the
unified log instead of the popover (see `EngineDiagnostics`).
Internal so the virtual-output level check in its sibling file
logs through the same object.

## engine-liveness-lag

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift`, `AppEvent.modeSelected`.

A mode-control segment was tapped. `monitorTargetError` is the
preview-target pre-flight (nil when the default output may receive
the preview; only sampled for Preview taps), and `isEngineRunning`
is the engine's live answer — the machine's belief can lag it by
up to one health-poll tick, and a tap in that window must rebuild
instead of toggling the monitor on a dead transport.

## health-polling-lifecycle

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.faultPollTask`.

Watches for engine faults/unexpected stops while a transport is
live. Owned here (not by the menu view) so faults are detected
even when the popover is closed; started/stopped by
`syncFaultPolling()` from the reducer's `hasLiveTransport`.

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.syncFaultPolling`.

Derives the health-poll task from the reducer state: it runs
exactly while a transport is live (including the failed-but-live
window after an engine fault, matching the teardown-bounded
polling this replaces).

## latency-reporting

Source: `crates/noican-core/src/stage.rs`, `Stage::latency_samples`.

Total internal latency in samples at [`ENGINE_SAMPLE_RATE`]
(resampling + frame buffering; excludes the model's own algorithmic
lookahead unless the implementation accounts for it).

Source: `crates/noican-core/src/stage.rs`, `FrameProcessor::output_delay`.

Additional algorithmic delay of the model itself, in samples at
[`Self::sample_rate`] (e.g. STFT window minus hop for overlap-add
models, or lookahead frames). Used only for latency reporting and
offline alignment; defaults to zero.

## level-meters

Source: `crates/noican-ffi/src/lib.rs`, `NoicanLevels::input`.

Decayed linear peak (0.0–1.0) of the model input (pre-processing),
measured per 10 ms worker block; 0.0 while the engine is stopped.

Reads one atomic without taking the control lock, so it never blocks —
regardless of what the control plane is doing.

Source: `crates/noican-ffi/src/lib.rs`, `NoicanLevels::output`.

Decayed linear peak (0.0–1.0) of the model output (post-processing),
measured per 10 ms worker block; 0.0 while the engine is stopped.

Reads one atomic without taking the control lock, so it never blocks —
regardless of what the control plane is doing.

## meters-outside-reducer

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.inputLevel`.

Peak meters, refreshed by `pollLevels()` only while the popover is
open. Display-only samples of lock-free engine atomics — not state
machine state, so they stay outside the reducer. Independent of
the Preview state: they move whenever the engine runs.

## output-underrun-counting

Source: `crates/noican-coreaudio/src/callback.rs`, `OutputFeed`.

The output side of a render callback on either transport: pops the
worker's output ring into the device buffer as dual mono
([`fill_dual_mono`]) and counts complete starvation as underrun.

A callback counts as an underrun only when it delivered no real sample
at all *after* the ring first produced audio. The ramp-up before that
is start-up latency, not underrun: the aggregate transport's ring
starts empty (the worker needs one full block of input before it
produces anything), so counting it would flag every model, including
ones that never miss the budget. Partial shortfalls are excluded on
purpose too: the worker writes 480-sample blocks into (typically)
smaller I/O periods, so a few benign partial zero-fills occur on every
model while the ring's phase cushion forms. A fully starved callback,
by contrast, means the worker fell a whole I/O period behind — audible
as dropouts in recordings from the virtual microphone.

Source: `crates/noican-coreaudio/src/callback.rs`, `OutputFeed::primed`.

Whether any sample has ever been popped. Callback-thread-only
state, hence no atomic.

Source: `crates/noican-coreaudio/src/callback.rs`, `OutputFeed::render`.

Renders one callback into `samples` (interleaved, `channels` per
frame); frames the ring cannot serve render silence. Never blocks;
an underrun is counted with a relaxed add (real-time safe).
