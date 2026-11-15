# 0001. Realtime audio thread rules

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the source
location of each comment at the time of the move (commit 397e3df)
is noted above its text.

## buffer-presizing-contract

Source: `crates/noican-models/src/factory.rs` lines 46–47.

Largest engine block the returned stages are pre-sized for (larger
blocks still work at the cost of a reallocation).

## lock-free-worker-observability

Source: `crates/noican-coreaudio/src/observe.rs` lines 1–12.

Lock-free peak level meters for the stream flowing through the
inference worker.

[`StreamLevels`] publishes per-block peaks of the model input
(pre-processing) and model output (post-processing) without ever
touching the meeting-facing path. It is updated on the inference
worker between device callbacks and obeys the same real-time rules as
the callbacks themselves (docs/tech-research.md §9): no allocation, no
locks — the meters are plain atomics.

The preview monitor's worker-side machinery (tee + feedback guard)
lives in [`crate::monitor`].

Source: `crates/noican-coreaudio/src/observe.rs` lines 16–21.

Per-block decay applied to the stored peak (simple exponential
ballistics on the engine side, so every reader sees the same motion).
At the 10 ms worker block size this is ≈ −0.9 dB per block, so a
full-scale peak falls below 1% (−40 dB) in about 440 ms — fast enough
to track speech pauses, slow enough that a 20 Hz UI poll never misses
a transient entirely.

Source: `crates/noican-coreaudio/src/observe.rs` lines 24–30.

Linear peak levels of the model input (pre-processing) and model
output (post-processing), shared lock-free between the inference
worker and the control plane.

Values are `f32` bit patterns in [`AtomicU32`]s; `Relaxed` ordering is
sufficient because each value is independently meaningful and a
slightly stale read is harmless for a meter.

Source: `crates/noican-coreaudio/src/observe.rs` lines 74–80.

The inference worker's per-block time budget in nanoseconds.

One engine block is 10 ms of audio ([`crate::WORKER_BLOCK_SAMPLES`] at
48 kHz), so a block that takes longer than 10 ms to process makes the
worker fall behind the device clock and eventually underruns the
output ring (the render callback then fills silence — audible as
dropouts in recordings from the virtual microphone).

Source: `crates/noican-coreaudio/src/observe.rs` lines 83–94.

Lock-free per-block processing-time statistics of the inference
worker, shared with the control plane for diagnostics.

Updated once per 10 ms engine block on the worker thread (three
relaxed atomic RMWs — the worker is not the audio callback, but it
still must never lock or allocate on this path so a slow stats write
can never *cause* the underruns it measures). Readers poll at UI
rates; slightly stale values are harmless for diagnostics.

Counters are cumulative since the last [`WorkerBlockStats::reset`]
(worker start/exit, or an explicit reset from the control plane when
the model changes and per-model attribution is wanted).

Source: `crates/noican-coreaudio/src/observe.rs` lines 151–152.

NaN-proof: `f32::max` keeps the accumulator when the operand is NaN,
so a corrupt sample cannot poison the meter.

## mach-time-constraint-policy

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1408–1409.

`thread_time_constraint_policy` (`mach/thread_policy.h`): all times
in mach absolute-time ticks; `preemptible` is a `boolean_t`.

## realtime-allocation-ban

Source: `crates/noican-core/src/capture.rs` lines 162–166.

Converts one chunk of native-rate samples, appending 48 kHz
samples to `output` (`input.len() × up / down` on average,
modulated by the drift correction). Allocation-free while `input`
stays within the preallocated `max_input_len` and `output` has
room for [`InputResampler::max_output_len`].

Source: `crates/noican-core/src/resample.rs` lines 436–438.

Upper bound on the samples one `process` call appends for
`input_len` input samples, at the largest drift correction. Reserve
the output `Vec` to this to keep `process` allocation-free.

Source: `crates/noican-coreaudio/src/callback.rs` lines 1–19.

Buffer geometry and ring hand-offs of the real-time callbacks.

The arithmetic that sizes the preallocated capture landing buffer and
bounds what a render callback may write, and the two hand-offs every
callback performs: queueing captured samples for the inference worker
([`CaptureFeed`]) and rendering the worker's output as dual mono
([`OutputFeed`], [`fill_dual_mono`]). Pure and platform-independent so
the workspace tests cover them; the AUHAL callbacks in the `macos`
module only apply them.

Real-time rules (docs/tech-research.md §9): a callback never
allocates, so the capture buffer is sized once at start from
[`MAX_CALLBACK_FRAMES`] — which is also set as
`kAudioUnitProperty_MaximumFramesPerSlice` on the unit — and a callback
asking for more frames than that is refused (fault flag, `paramErr`)
rather than served from a fresh allocation. The hand-offs only move
`f32` samples through preallocated lock-free rings and bump relaxed
atomic counters: ring overrun drops samples, underrun renders silence,
and neither ever blocks.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 170–172.

A delay line holding `len` zeros with room for `capacity` samples, so
the first frame's pushes (which precede the pops) never reallocate on
the inference thread.

## realtime-audio-rules

Source: `crates/noican-coreaudio/src/macos/monitor.rs` lines 436–440.

Render callback of the output-only monitor AUHAL. Real-time rules
(docs/tech-research.md §9): it only moves `f32` samples from the
preallocated monitor ring into the device buffer — no allocation, no
locks, no syscalls. Ring underrun renders silence and re-arms priming;
it never blocks and never touches the meeting-facing path.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 276–277.

Bound the callback size so the preallocated render buffer always
suffices (the callback never allocates).

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 357–360.

Inference worker of the split transport: drains native-rate capture
samples, converts them to 48 kHz with the drift-compensated
resampler, and runs the unchanged engine in 10 ms blocks. All state
is preallocated; the loop never locks (docs/tech-research.md §9).

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 434–439.

Input callback of the capture AUHAL: renders the microphone's frames
into the preallocated buffer and pushes them to the capture ring
([`Capture`], shared with the aggregate render callback).
Real-time rules (docs/tech-research.md §9): no allocation, no locks;
ring overrun drops samples; a render failure flags the fault and
returns cleanly.

Source: `crates/noican-coreaudio/src/macos/split.rs` lines 461–468.

Render callback of the output-only AUHAL on the virtual output: pops
engine-rate samples from the output ring into the device buffer,
duplicating the mono engine signal into every client channel of each
frame (as many as the render format set in [`configure_output_auhal`]
— the width is taken from the buffer AUHAL hands over, so a 1- or
2-channel virtual output runs the same code). Underrun renders
silence — it never blocks (docs/tech-research.md §9); systematic
underrun is prevented by the drift servo, not here.

## realtime-block-budget

Source: `crates/noican-cli/src/process.rs` lines 26–34.

[`run_stage_aligned`] that also returns the wall-clock time of every
`process_block` call in nanoseconds (one entry per [`BLOCK_LEN`]
block, in order). This is the same measurement `block_bench` makes,
taken on real audio instead of noise, so the evaluation table can
show whether a candidate fits the live worker's budget.

# Errors

Propagates stage processing failures.

Source: `crates/noican-models/examples/block_bench.rs` lines 1–16.

Per-block processing-time measurement for one model stage.

Feeds 10 ms blocks (480 samples at 48 kHz — exactly what the live
inference worker hands to the engine) through a stage and reports the
block-time distribution against the real-time budget. This is the
measurement behind the single-thread decision documented on
`onnx::load_streaming_session` and the budget numbers recorded in
docs/macos-hardware-test.md.

Usage:

```bash
NOICAN_MODELS_DIR=/tmp/noican-models \
  cargo run --release -p noican-models --example block_bench -- \
  dpdfnet8 60
```

Source: `crates/noican-models/examples/block_bench.rs` lines 29–30.

The real-time budget per block, mirroring
`noican_coreaudio::BLOCK_BUDGET_NS`.

## realtime-cadence

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1438–1452.

Guaranteed computation per period, sized on the heaviest model's
typical block: p50 ≈ 4.2 ms on a modest 4-core x86-64 host
(`noican-models/examples/block_bench.rs`, 2026-09-02). This is the
CPU slice the scheduler reserves, not a ceiling — with `preemptible`
set the thread may keep running past it up to `constraint` — so p50
(plus margin) rather than that host's p95 of 6.2 ms is the basis:
tail blocks still complete within the period, Apple Silicon
performance cores are faster than the measuring host, and a larger
reservation would only raise the admission cost of the RT band.
Persistent overruns of the declared computation risk demotion by
XNU; what the 2026-09-04 on-device run supports is the absence of
steady-state underruns (block-time figures only reach the log
alongside underrun growth, so the device logged none for clean
models) — the sub-5 ms steady-state figure itself is the
`block_bench` measurement above.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1454–1455.

Deadline within each period: the whole period, matching the audio
cadence the output ring absorbs.

## realtime-thread-rules

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 545–547.

Timing the block on the worker (not a callback): `Instant::now`
is a non-blocking clock read, fine on the inference thread that
runs whole models.

## realtime-wakeup-semaphore

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 208–212.

Owned libdispatch semaphore. Wakes the inference worker when the render
callback has produced samples, so the worker blocks between device
callbacks instead of busy-spinning (`dispatch_semaphore_signal` is the
mechanism Apple's audio-workgroup example uses from real-time threads:
it never blocks or allocates).

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 216–217.

The semaphore handle is shared with the render callback and the worker
thread; libdispatch semaphores are internally thread-safe.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1556–1559.

Block until the render callback signals more input (or the
timeout elapses, so shutdown is always noticed). Busy-spinning
here would burn a core for the whole session and distort the
os_workgroup's power/deadline balancing.

## rosetta-budget-attribution

Source: `macos/Sources/NoicanMenuBar/EngineDiagnostics.swift` lines 40–42.

True when the process runs translated under Rosetta 2 — inference
would be silently slower, so hardware budget numbers must carry
this bit.

## threading-and-allocation-rules

Source: `crates/noican-coreaudio/src/macos/monitor.rs` lines 328–329.

UID of a device, or an empty string when unreadable. Runs on the
control thread only (allocates; `CFString` round-trip).

## worker-block-processing

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 538–543.

Runs the engine on one block and fans the result out to the
meters, the preview tee, and the output ring — the per-block step
shared by the aggregate and split worker loops — and records its
duration in the block statistics. An engine failure emits silence
and flags the fault; output-ring overrun drops samples rather
than blocking.

## worker-block-size

Source: `crates/noican-coreaudio/src/lib.rs` lines 73–74.

Samples per engine block driven by the inference worker (10 ms at the
48 kHz engine rate; matches the CLI comparison block size).

## worker-realtime-promotion

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 449–454.

Whether the inference worker's real-time promotion succeeded at
loop start. False until the worker records it; afterwards it keeps
that start-time result — it is not live scheduling-band membership
(XNU may demote a thread that persistently overruns its declared
computation), and a stopped-but-not-dropped runtime retains the
last start's value.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 508–516.

Worker setup shared by both loops, run on the worker thread:
meters and statistics start from zero, the thread is promoted to
real-time scheduling, and it joins the device's audio workgroup.

The promotion must precede the join (see
[`promote_current_thread_to_realtime`]); its failure degrades, not
faults, and the stored value is the result at loop start (XNU may
later demote a persistently overrunning thread, which the flag
does not track). A failed join flags the fault.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 879–885.

Diagnostic: whether the inference worker's mach time-constraint
(real-time) promotion succeeded at loop start (see
`promote_current_thread_to_realtime`). False means the worker
runs at default priority, so budget misses on hardware may be
scheduling, not model cost. This reports the start-time result,
not live membership: XNU may later demote a persistently
overrunning thread without this flag changing.

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1458–1472.

Promotes the calling thread to mach time-constraint ("real-time")
scheduling, the setup Apple's audio-workgroup guidance requires for
self-created inference threads *before* they join the device's
workgroup: joining alone conveys the deadline but does not lift the
thread out of its default priority, so the scheduler may still park it on
efficiency cores or preempt it for tens of milliseconds — observed on
hardware (2026-09-02) as chronic 10 ms-budget misses for the heaviest
model then registered (max 80.5 ms) and one-shot 40 ms stalls even on
light models, on a machine where the same models measure well inside the
budget when scheduled.

Returns whether the promotion succeeded; a failure is survivable
(audio still flows at default priority) and is surfaced through
[`Runtime::worker_realtime`] so hardware runs can interpret their
underrun numbers.

## worker-wait-timeout

Source: `crates/noican-coreaudio/src/macos/mod.rs` lines 1397–1399.

Longest the worker sleeps waiting for the render callback's semaphore
signal before re-checking the shutdown flag (a fraction of the ~5.3 ms
device period at 256 frames / 48 kHz).
