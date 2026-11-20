# 0007. DSP stage design

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the source
location of each comment at the time of the move (commit 397e3df)
is noted above its text.

## click-free-stage-switching

Source: `crates/noican-core/src/switch.rs` lines 1–17.

Lock-free publication and click-free activation of prepared stages.

The control plane (UI / model-loader thread) fully constructs the next
[`Stage`] — including weight loading and inference-session setup — and
hands it to the inference thread through a bounded lock-free queue
([`StagePublisher::publish`]). The inference thread ([`SwitchingEngine`])
fades the current stage to silence, swaps ownership, and fades the new
stage in, so a model switch never clicks and never blocks the audio path
(docs/tech-research.md §9).

The engine also owns the dry/wet intensity blend ([`crate::mix`]):
it runs *before* the switch fade gain, so during a model switch the
whole mixed signal — wet and delay-compensated dry alike — fades out
and back in. That ordering is what lets the dry-compensation delay
jump to the new stage's latency exactly at the silent fade boundary,
so switching stages with different latencies stays click-free at any
intensity.

Source: `crates/noican-core/src/switch.rs` lines 33–36.

Producer-side handle used by the UI or model-loader thread.

The queue has one preallocated slot: publishing a newer stage replaces an
older unconsumed stage without locking the inference thread.

Source: `crates/noican-core/src/switch.rs` lines 52–54.

Publishes a fully initialized stage for activation by the inference
thread. Returns the id of a previously published stage that was
superseded before the inference thread consumed it, if any.

Source: `crates/noican-core/src/switch.rs` lines 70–74.

Inference-thread owner of the active stage.

Retrieving a published replacement requires no mutex and no heap
allocation; the pending stage is warmed (fed input) while the current one
fades out, so its internal pipeline is primed at activation time.

Source: `crates/noican-core/src/switch.rs` lines 96–98.

Longest supported fade half. Bounds the fade so the internal gain
arithmetic is exact (`u16` → `f32` is lossless); ~1.37 s at 48 kHz,
orders of magnitude above any sensible switch fade.

Source: `crates/noican-core/src/switch.rs` lines 102–115.

Creates a switching engine and its producer-side publisher.

`fade_samples` is the length of each fade half (out and in) in
samples at [`crate::stage::ENGINE_SAMPLE_RATE`]; `max_block_len`
pre-sizes the warmup buffer (larger blocks still work at the cost of
a reallocation on the control-of-flow path, not the audio callback).
`intensity` is the shared dry/wet control the engine's mixer reads
each block; the caller keeps a clone to move the slider without
touching the engine.

# Errors

Returns [`StageError::Unsupported`] when `fade_samples` is zero or
exceeds [`MAX_FADE_SAMPLES`].

Source: `crates/noican-core/src/switch.rs` lines 165–176.

Processes one block at the engine rate and advances any switch
transition. `output.len()` must equal `input.len()`.

The block passes through three steps in order: the active stage
(wet), the dry/wet intensity blend (dry delayed by the *active*
stage's reported latency), then the switch fade gain — so a
switch fades the complete mix, and the dry-compensation delay
only ever changes at the silent fade boundary.

# Errors

Propagates [`StageError`] from the active or warming stage.

Source: `crates/noican-core/src/switch.rs` lines 231–233.

`prepared` (now holding the superseded stage) is dropped here, on
the inference thread — allowed, since only the audio I/O callback
is allocation-free (docs/tech-research.md §9).

Source: `crates/noican-core/src/switch.rs` lines 238–240.

Exact fade gain: both operands fit `u16` (enforced by
[`MAX_FADE_SAMPLES`] in [`SwitchingEngine::new`]) and `u16` → `f32` is
lossless, so no precision-loss opt-out is needed.

Source: `crates/noican-core/src/switch.rs` lines 426–429.

The dry-compensation delay jumps from the old stage's latency
to the new one's exactly at the silent fade boundary; on a
steadily rising input any unmasked jump would appear as a
step. Bounded sample-to-sample deltas prove the fade masks it.

## composite-stage-dependencies

Source: `crates/noican-models/src/factory.rs` lines 54–56.

Path of file `index` of the registry entry `spec` depends on
([`ModelSpec::depends_on`], one level), for composite stages that
run another entry's weights.

## core-engine-architecture

Source: `crates/noican-core/src/lib.rs` lines 1–26.

Core audio-processing abstractions for the Noican engine.

This crate is platform-independent and free of inference-runtime
dependencies. It defines the common stage interface that every
noise-suppression / speaker-suppression model implements, plus the
sample-rate and frame-size adaptation that lets the engine treat all
models identically (docs/tech-research.md §12, Phase 0).

- [`stage::Stage`]: streaming processor at the fixed engine rate
  ([`stage::ENGINE_SAMPLE_RATE`], 48 kHz), arbitrary block sizes.
- [`stage::FrameProcessor`]: what a model actually implements — fixed
  frame size at its native rate.
- [`framed::FramedStage`]: adapts a `FrameProcessor` to a `Stage`,
  handling resampling (integer factors) and frame accumulation.
- [`resample`]: streaming polyphase FIR decimator/interpolator
  (integer factors, model path) and the arbitrary-ratio polyphase
  resampler (capture path).
- [`capture`]: native-rate microphone capture support — the input
  resampler (any native rate to 48 kHz, drift correction folded in)
  and the ring-occupancy drift servo for non-48 kHz microphones
  (issue #7).
- [`switch::SwitchingEngine`]: lock-free, click-free runtime switching
  between prepared stages (the live-pipeline building block).
- [`mix::IntensityControl`]: the atomic dry/wet ("strength") control
  whose blend runs inside the switching engine, with the dry path
  delay-compensated by the active stage's reported latency.

## dry-wet-intensity-mix

Source: `crates/noican-core/src/mix.rs` lines 1–23.

Dry/wet intensity blending with a latency-compensated dry path.

The intensity ("strength") control blends the processed signal (wet)
with the raw input (dry): 100% is fully processed, 0% is the raw
microphone. The blend runs on the inference thread inside
[`crate::SwitchingEngine::process_block`] — upstream of the output
ring and the preview monitor tee, so every consumer hears the same
mix — and obeys the real-time rules (docs/tech-research.md §9):

- The intensity value crosses threads through **one atomic**
  ([`IntensityControl`], an `f32` bit pattern in an `AtomicU32`).
  Changing it never rebuilds or locks anything.
- The blend itself performs no allocation and no locking: the dry
  history lives in a ring buffer preallocated at construction time
  (control plane), and per-block work is a fixed multiply-add pass.

Naively summing dry and wet would comb-filter (double voice): the wet
signal carries the stage's algorithmic and resampling latency
(10–30 ms class). The mixer therefore delays the dry signal by the
active stage's [`crate::Stage::latency_samples`] before blending, so
both paths line up. Intensity changes are additionally smoothed with
a per-block linear ramp, so a slider drag never steps ("zipper"
noise).

Source: `crates/noican-core/src/mix.rs` lines 28–32.

Capacity of the dry-compensation ring in samples at the engine rate.
A power of two (mask indexing, no division on the audio path) covering
~1.37 s at 48 kHz — an order of magnitude above any streaming stage's
reported latency. Larger reported latencies are clamped to this bound
(partial compensation instead of an out-of-range read).

Source: `crates/noican-core/src/mix.rs` lines 35–40.

Control-plane handle to the dry/wet intensity: one shared atomic.

`1.0` (the default) is fully processed output; `0.0` is the raw
microphone. Cloning shares the same underlying value, so the control
plane keeps one handle while the inference thread's mixer reads the
other — no lock, no rebuild, safe to update at UI slider rates.

Source: `crates/noican-core/src/mix.rs` lines 47–48.

Creates a control initialized to `initial` (clamped to `0.0..=1.0`;
a non-finite value falls back to fully processed).

Source: `crates/noican-core/src/mix.rs` lines 58–61.

Publishes a new intensity, clamped to `0.0..=1.0`. Non-finite
values are ignored (the previous intensity stays), so a corrupt
caller value can never poison the audio path. One atomic store —
never blocks, safe from any thread.

Source: `crates/noican-core/src/mix.rs` lines 79–80.

Fully processed (100%): the engine behaves exactly as it did
before the intensity control existed until someone moves it.

Source: `crates/noican-core/src/mix.rs` lines 86–90.

Inference-thread side of the intensity control: blends the delayed dry
signal into the wet block in place.

Owned by [`crate::SwitchingEngine`]; not constructed on the audio
path (the ring allocation happens on the control plane).

Source: `crates/noican-core/src/mix.rs` lines 115–124.

Blends `dry` (the raw engine-rate input) into `wet` in place:
`wet[i] = w * wet[i] + (1 - w) * dry[i - dry_delay]`, with `w`
ramping linearly across the block toward the control's value.

`dry_delay` is the active stage's reported latency in samples;
it aligns the dry path with the wet path so a partial mix does
not comb-filter. The dry history is fed continuously (even at
100% wet), so lowering the intensity mid-stream blends against
real history, not silence. Real-time safe: no allocation, no
locks, one atomic read per block.

Source: `crates/noican-core/src/mix.rs` lines 131–133.

Lossless `u16 → f32` ramp arithmetic, mirroring `switch::ratio`
(blocks are 10 ms; anything above 65535 samples still ramps,
just marginally faster than one block).

Source: `crates/noican-core/src/mix.rs` lines 139–141.

For blocks over u16::MAX samples the saturated `len` makes the
per-sample step too large, so clamp to the ramp segment — the
block lands exactly on `target` instead of overshooting.

Source: `crates/noican-core/src/mix.rs` lines 149–150.

At intensity 1.0 this is exactly `wet` (adding a true zero),
so full strength is bit-identical to the pre-mixer engine.

Source: `crates/noican-core/src/mix.rs` lines 153–154.

Land exactly on the target: the per-sample accumulation may
carry rounding, and the next block must start from the truth.

## dry-wet-strength-contract

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift` lines 165–170.

Publishes the dry/wet strength (0 = raw microphone, 1 = fully
processed; out-of-range values clamp, non-finite values are
ignored). One atomic store — never blocks, never rebuilds the
engine, safe at slider rates and while stopped (the value seeds
the next start). The same mix feeds the virtual microphone and
the preview monitor.

## dsp-window-semantics

Source: `crates/noican-models/src/dsp.rs` lines 8–11.

Vorbis window: `w[i] = sin(pi/2 * sin^2(pi/2 * (i + 0.5) / (N/2)))`.

Satisfies squared-COLA at 50% overlap, so analysis + synthesis
windowing needs no extra normalization (DPDFNet, DeepFilterNet family).

Source: `crates/noican-models/src/dsp.rs` lines 29–30.

Periodic Hann window (`torch.hann_window` semantics):
`w[n] = 0.5 * (1 - cos(2*pi*n / N))` — divisor `N`, not `N - 1`.

Source: `crates/noican-models/src/dsp.rs` lines 47–48.

Square root of the periodic Hann window (`hann_sqrt` in DPDFNet
metadata for some profiles).

## fft-interleaved-layout

Source: `crates/noican-models/src/dsp.rs` lines 100–105.

Forward transform of `frame` (length `n`), writing interleaved
`[re0, im0, re1, im1, ...]` into `spec` (length `2 * bins`).

# Errors

Returns [`StageError::Inference`] on an internal FFT failure.

Source: `crates/noican-models/src/dsp.rs` lines 126–131.

Inverse transform of interleaved `spec` (length `2 * bins`) into
`frame` (length `n`), scaled by `1 / n` (numpy `irfft` semantics).

# Errors

Returns [`StageError::Inference`] on an internal FFT failure.

Source: `crates/noican-models/src/dsp.rs` lines 141–141.

realfft requires the Nyquist/DC imaginary parts to be zero.

## fft-scaling-semantics

Source: `crates/noican-models/src/dsp.rs` lines 54–55.

A paired forward/inverse real FFT of a fixed size with numpy
`rfft`/`irfft` scaling: forward is unnormalized, inverse divides by `n`.

## framed-stage-adapter

Source: `crates/noican-core/src/framed.rs` lines 1–7.

Adapter turning a fixed-frame, fixed-rate [`FrameProcessor`] into a
[`Stage`] operating at [`ENGINE_SAMPLE_RATE`] with arbitrary block sizes.

This is where per-model sample-rate and frame-size differences are
absorbed (docs/tech-research.md §12): the engine always exchanges 48 kHz
blocks; the adapter resamples (integer factor), accumulates full model
frames, and re-emits at the engine rate with a fixed, reported latency.

Source: `crates/noican-core/src/framed.rs` lines 32–39.

Wraps `processor`. `max_block_len` is the largest block the engine
will pass to [`Stage::process_block`] (used only to pre-size buffers;
larger blocks still work at the cost of a reallocation).

# Errors

Returns [`StageError::Unsupported`] when the processor's sample rate
does not divide [`ENGINE_SAMPLE_RATE`].

## framed-stage-latency

Source: `crates/noican-core/src/framed.rs` lines 56–58.

Latency: one full round of buffering, plus both filters' group
delays when resampling. Priming the output FIFO with `round_len`
zeros guarantees process_block can always pop a full block.

## hush-absolute-level

Source: `crates/noican-cli/src/eval_run.rs` lines 108–112.

A warning line when `samples` exceed full scale: every written WAV is
clamped to ±1.0 on the way to 16-bit, so the listening set would be
clipped while the metrics (computed on the unclamped floats) stay
clean. Nothing is rescaled here — Hush's behaviour depends on the
absolute level, so the level the user chose must reach the model.

## hush-band-gain-window

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 156–167.

STFT bins of Hush's spectrum whose gain steers the added band:
4.0–7.0 kHz inclusive (`bin × 16 000 / 320` Hz, 50 Hz per bin).

Below 4 kHz Hush's ERB bands follow individual formants and pitch
harmonics — decisions that say little about the 8–24 kHz band. Above
≈ 7.2 kHz the decimation lowpass (cutoff 0.45 × 8 kHz) has already
rolled the model's input off, so those bins carry little energy. The
octave in between is where the model's judgement most resembles what
it would do to the band being reconstructed. Tuning variants
(2–7 kHz, 6–7.2 kHz, and a 30 ms release) were measured with `noican
eval`; the results are recorded in the pull request that introduced
this stage.

## hush-core-contract

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 144–145.

Hush's native rate; the core is rejected at construction if the
tarball says otherwise.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 314–321.

Loads the Hush core from its tarball (the `hush` registry entry's
file) and wraps it.

# Errors

Returns [`StageError::Inference`] when the tarball cannot be loaded
and [`StageError::Unsupported`] when it is not the 16 kHz /
160-hop model this wrapper is built around.

## hush-failure-frame-handling

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 378–382.

`apply` and `split` have queued this frame's per-sample state;
`merge` and `restore` must consume it even when the core fails,
or the delay lines run one frame long for the rest of the
session (the worker treats an inference error as one silent
block and keeps calling). A failed frame is a muted frame.

## hush-high-band-gating

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 675–679.

With real weights: the gate closes. A 12 kHz tone lies entirely
in the band the core never sees, so the core's input is below its
silence threshold, `band_gain` is 0, and the tone must not reach
the output — an ungated band-split (option A in the module docs)
would pass it at full level.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 704–706.

The tone's abrupt onset is broadband and leaks through the
decimator for the first frame; the gate must be shut from the
second frame on.

## hush-input-leveler

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 476–476.

0.6× the reference signal: ≈ −12 dBFS, 25 dB above the target.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 778–781.

With real weights: hot input reaches the core at the target level
and leaves the stage at the input's own level (plus the makeup
gain), so the leveler changes what Hush decides, not how loud the
stage is.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 798–798.

Speech-like tone pair at ≈ −20 dBFS: 17 dB above the target.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 874–877.

Band-limit the test signal the same way the 16 kHz path does,
so the high band the wideband stage would add is ~zero. At
0.03× (≈ −38 dBFS) it sits under the leveler's target, so the
two stages feed the core the same samples.

## hush-makeup-gain

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 16–45.

Fixed makeup gain applied to Hush's output, in dB.

Hush's network attenuates speech, not just noise. Measurement (same
discipline as `LOOKAHEAD_HOPS` in the DPDFNet stage — an empirical
constant with its measurement recorded): the aligned CLI batch
(espeak-ng speech → 48 kHz mono → `process` → voiced-frame RMS over
10 ms frames whose own level exceeds −50 dB, compared against
passthrough) measured a **−3.4 dB to −1.5 dB voiced-frame RMS deficit
depending on the speech material and level** (−3.4 dB on the
2026-08-31 hardware-follow-up corpus at 0.70 peak; −2.5/−2.0/−1.9 dB
on two other espeak voices at 0.70 peak; −1.5 dB at 0.35 peak —
`x86_64` CLI builds). UL-UNAS on the same 16 kHz resampling path
measured ±0.4 dB, so the deficit is Hush's own gain characteristic,
not the rate conversion. This constant is the midpoint of the
measured deficit range. The authoritative acceptance bound is the
**±1 dB band around passthrough** (what the unit test pins): over
the recorded deficits above the midpoint predicts residuals of
−0.95 dB to +0.95 dB, and end-to-end re-measurement on the three
re-runnable materials landed at +0.5, −0.1 and +0.97 dB — the
prediction and the measurements all inside the band.

No clipping guard follows the gain, by design. The worst measured
output/input peak ratio after the gain is ≈1.16 (raw Hush peak 0.61
on a 0.70-peak input, ×1.33), so input peaking above ≈0.86 can push
isolated output samples past 0 dBFS in the all-`f32` engine path.
That path has no hard ceiling (floats do not wrap; downstream
consumers clamp on conversion), a microphone captured that hot is
already at its own clipping point, and a limiter here would distort
every loud syllable to guard a corner case — so the overshoot is
accepted and documented instead.

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 65–66.

Applies a fixed linear gain in place (the Hush makeup gain hook;
separated from the model call so it is testable without weights).

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 140–141.

Linear output gain (1.0 for DeepFilterNet3; the measured makeup
gain for Hush — see [`HUSH_MAKEUP_GAIN_DB`]).

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 207–213.

Hush without the makeup gain, for composite stages that apply
`HUSH_MAKEUP_GAIN_DB` themselves after mixing in other material
(see `stages::hush_wideband`).

# Errors

Returns [`StageError::Inference`] when the tarball cannot be loaded.

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 308–309.

A pure per-sample gain: latency, alignment, and the
cross-correlation residual are unaffected.

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 342–345.

The measured voiced-frame RMS deficits across the corpus in
the constant's doc comment (dB versus passthrough). The
constant must bring every one of them into the ±1 dB
acceptance band around passthrough.

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 355–358.

The no-clipping-guard design decision: the worst measured raw
output peak (0.61 on a 0.70-peak input) must stay under full
scale after the gain — overshoot only enters with inputs
already at their own clipping point (see the doc comment).

Source: `crates/noican-models/src/stages/dfn_tract.rs` lines 419–421.

With real weights, the Hush stage output must be exactly the
gain-1.0 stage output scaled by the makeup gain — the gain is a
pure post-multiply, so alignment and residual delay are untouched.

## hush-wideband-band-split

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 1–132.

Hush with 48 kHz output: a band-split wrapper around the 16 kHz Hush
core (`hush-48k` in the registry).

Hush is trained and shipped at 16 kHz only, so the plain `hush` stage
runs it through the 3:1 [`Decimator`] / [`Interpolator`] pair and
everything above 8 kHz is gone from the output. This stage keeps the
same core and the same latency, and adds the input's own upper band
back — gated by what Hush decided for the adjacent band:

```text
x ─ × g_in(t) ─┬─ decim(3) ─ Hush(16 kHz) ─ interp(3) ───────────────┐
               │        │                                            ├─(+)─ ÷ g_in(t−600) ─ × makeup ─ out
               │        └─ interp(3) ─(−)─ delay(480) ─ × g_hi(t) ───┘
               └───── delay(120) ────┘
```

`g_in` is the input leveler's trim (see `stages::leveler`): the
loudest sustained talker is brought down to the level at which Hush
passes a lone voice at parity, and the trim is undone on the output,
so only what the core sees changes. Reading `x` as the trimmed input:
`x delayed 120 − interp(decim(x))` is the part of the input the model
never saw (the 8–24 kHz band plus the resampler's transition region),
exactly time-aligned with what it did see, so when the core is the
identity and `g_hi = 1` the stage reduces to a pure 120-sample delay
(a unit test pins this). `g_hi` is the broadband gain Hush applied to
the top of its own band (`BAND_GAIN_BINS`, 4–7 kHz), read from the model's
noisy and enhanced spectra after each frame (see
`DfTractStage::band_gain`) and ramped linearly across the 10 ms
frame. When Hush mutes a frame the added band is muted with it; when
Hush passes the frame the band passes at Hush's own gain.

# Decision record (2026-09-11)

Options weighed for getting Hush's suppression at 48 kHz output
quality, against the axes in docs/tech-research.md §6.4 (high-band
retention, suppression preserved — above all no high-band leak —
own-voice fidelity, added latency and 10 ms block cost,
implementation and maintenance cost, dependencies and licences,
fail-safe behaviour, compatibility with the lock-free switch and the
dry/wet compensation):

- **Status quo** (`hush`, 16 kHz output): keeps the suppression, loses
  everything above 8 kHz (`HF keep` ≈ −80 dB on the harness). Rejected
  as the default candidate; the entry stays for comparison.
- **A. Band-split with an ungated high band**: transparent above
  8 kHz but leaks the interferer's sibilants whenever Hush mutes —
  the one failure the evaluation treats as disqualifying. Rejected on
  its own, kept as the frame of the chosen design.
- **B. Sidechain, external gain**: run a separate STFT on the 16 kHz
  input and output and derive a gain. Works but duplicates the
  model's analysis, and its frame alignment with Hush's internal
  STFT would be approximate. Superseded by C′.
- **C. Sidechain via the model's mask (fork `deep_filter`)**: expose
  the ERB mask. Rejected: the upstream crate already exposes the
  post-mask spectra publicly (`DfTract::get_spec_noisy` /
  `get_spec_enh`, `libDF/src/tract.rs`), so no fork is needed.
- **C′ (chosen, with A's frame)**: gate the input's high band with the
  gain Hush applied in 4–7 kHz, read from those public spectra. No
  extra analysis, exact frame alignment, zero added latency, no new
  dependency, and the fail-safe is the plain `hush` output (gain 0 →
  identical signal up to the makeup gain).
- **D. Run Hush at 48 kHz by configuration**: rejected on inspection —
  the tarball's `config.ini` fixes the STFT (`fft_size` 320, `hop`
  160, `sr` 16000), the ERB filterbank and the network input widths;
  `DfParams` only reads them.
- **E. Wait for an upstream 48 kHz Hush**: checked 2026-09-11 —
  `weya-ai/hush` on Hugging Face ships the 16 kHz tarball only and its
  README lists a retrain as "coming soon"; `pulp-vision/Hush` has one
  release (v1.0.0, 2026-03-20) and its 2026-07-18 commit *removed* an
  embedded 48 kHz DeepFilterNet3 (not Hush). Nothing to adopt; the
  registry can add it as another entry if it ever lands.
- **F. Neural bandwidth extension after Hush**: no streaming,
  sub-10 ms CPU model with a permissive licence is available, and a
  generative upper band is the wrong tool when the true upper band is
  in the input. Rejected.

A second exploration round (heterodyning a second Hush pass onto the
upper band; mapping the ERB mask through `erb_fb` per bin; running the
tract graphs at 48 kHz) produced nothing better than C′ and stopped.

**High-band source** (dry input vs. a light 48 kHz denoiser): the dry
input adds no latency and no second weight file; a denoiser in the
upper path would push the stage's latency to its own delay plus
alignment (≈ 31 ms total for a ~21 ms denoiser) and roughly double
the block cost. The dry route is chosen; if hiss in the upper
band proves audible in listening, a denoised upper path is the
documented follow-up.

**Makeup gain**: the core runs without `HUSH_MAKEUP_GAIN_DB` and the
gain is applied once to the recombined output, so both bands share
Hush's measured level characteristic (the high band is scaled by
Hush's own gain via `g_hi`, hence needs the same correction).

# Decision record: input level (2026-09-24)

Hush reads the absolute input level as a primary-speaker cue
(harness sweep, `docs/hush-48k-eval.md` "Level dependence": own-voice
SI-SDR 14.4 dB and level +0.4 dB at −37 dBFS; 4.5 dB / −4.6 dB at
−30; 1.7 dB / −7.8 dB at −22; 4.7 dB at −45; everything zeroed at
−55). On a 36-minute owner recording through this stage the output's
share above 8 kHz fell 9 dB between the quietest and loudest
sentences (−39 dB at −45…−40 dBFS, −48 dB at −30…−20), i.e. the
brightness followed the sentence dynamics. Options:

- **Status quo**: the measured cause; rejected.
- **Automatic gain control (boost and cut) in this stage**: brings a
  quiet talker up too — and the same sweep shows Hush passing a lone
  voice at −37…−49 dBFS as the primary speaker while suppressing one
  at −52 by 18 dB, so boosting a quiet second talker during the
  owner's pauses would hand them the microphone. Rejected.
- **Attenuation-only leveler anchored to the loudest sustained
  talker (chosen)**: `stages::leveler`. Input at or below the target
  is untouched (the live-accepted regime); hotter input is trimmed
  to the target with a slew-limited anchor and the trim is undone on
  the output. A quieter second talker can at most release the trim
  back to 0 dB, i.e. to the status quo.
- **Level the core inside `DfTractStage` for `hush` too**: one place
  for both entries, but it would change the `hush` entry the
  listening test compares against and whose makeup gain and
  acceptance were measured without it. The leveler is a standalone
  struct so `hush` can adopt it in a later change if wanted.
- **A user-facing input trim in the app**: would fix the level once
  per session by hand; needs Swift work and puts a calibration step
  on the user. Rejected while the automatic trim holds.
- **Upstream fix (a level-robust Hush)**: nothing to adopt; see the
  2026-09-11 record.

Second round (per-frame normalisation — flattens the dynamics Hush
uses and lifts pauses to speech level; a 60 s level histogram — same
drift onto a lone second talker as AGC, more state) produced nothing
better and stopped. Frames the core still skips as silent are trimmed
input under −70 dBFS; the zero output there is accepted as before.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 179–184.

Splits the engine-rate input into Hush's band and the remainder, and
recombines Hush's output with the (gated) remainder.

Pure DSP, no model: the core sits between [`Self::split`] and
[`Self::merge`] so the reconstruction can be tested with an identity
core.

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 236–237.

Consumes one 480-sample frame: returns the 160-sample low band for
the core and queues the aligned high band for [`Self::merge`].

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 288–292.

Hush with the input's upper band added back under Hush's own gate.

A 48 kHz [`FrameProcessor`] with 480-sample frames; wrapped by
[`noican_core::FramedStage`] it reports the same latency as `hush`
(1080 samples, 22.5 ms).

Source: `crates/noican-models/src/stages/hush_wideband.rs` lines 903–904.

The residual high band of a band-limited input is the
resampler's stopband (−60 dB and below).

## intensity-change-ungated

Source: `macos/NoicanState/Sources/NoicanState/AppEvent.swift` lines 24–28.

The strength slider moved (0 = raw microphone, 1 = fully
processed). Applying it is a lock-free atomic write on the engine
side, so this claims no transition and is accepted even while
busy — gating it would make the slider feel dead during a model
download.

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 310–314.

Dry/wet mix applied inside the inference worker as one
atomic value: dragging never rebuilds the engine and
stays live during busy transitions, so the slider is
deliberately not disabled with the pickers. Preview
plays the same mix the virtual microphone receives.

## intensity-dry-wet-mix

Source: `crates/noican-ffi/src/lib.rs` lines 825–840.

Sets the dry/wet intensity ("strength").

1.0 is fully processed output, 0.0 is the raw microphone, and values
between blend the two with the dry path delay-compensated by the
active model's reported latency (so a partial mix does not
comb-filter).

One atomic store — never blocks, never rebuilds the engine, safe at
UI slider rates and while stopped (the value carries into the next
start). Out-of-range values are clamped; non-finite values are
ignored. The same mix feeds the virtual microphone and the preview
monitor. Returns 0, or -1 for a null handle.

# Safety

`handle` must be null or a live engine handle.

## intensity-lock-free-atomic

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 175–178.

Dry/wet strength (0 = raw microphone, 1 = fully processed;
default 1). Slider state like the pickers — it persists across
engine lifecycles and applying it is a lock-free atomic write,
so changes never claim an engine transition.

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 162–164.

Strength slider binding setter (0 = raw microphone, 1 = fully
processed). Not busy-gated: applying it is a lock-free atomic
write, so the slider stays live even during a model download.

## intensity-slider-semantics

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 162–166.

The strength slider is picker-like state with an atomic-write
effect: it claims no engine transition (the engine applies it
lock-free mid-stream), so it is accepted even while busy and
regardless of the engine phase — the value persists across engine
lifecycles and seeds the next start.

## leveler-delay-contract

Source: `crates/noican-models/src/stages/leveler.rs` lines 187–189.

A leveler for `frame_len`-sample frames at `sample_rate`, whose
restore runs `delay` samples after the apply (the wrapped core's
total output delay at that rate).

## leveler-design

Source: `crates/noican-models/src/stages/leveler.rs` lines 1–57.

Input leveler for level-sensitive cores: trims the input so the
loudest sustained talker sits at a target level, and undoes the trim
on the output so the stage's overall level is untouched.

Hush's network reads the *absolute* input level as a cue for who is
the primary speaker (measured in `docs/hush-48k-eval.md`, "Level
dependence"): a lone voice passes at parity around −37 dBFS RMS,
loses level and its 4–7 kHz band progressively above that (−4.6 dB at
−30, −8.8 dB at −15), and is treated as background below ≈ −45. In a
live session the talker's level moves with their voice and mic
distance, so the output brightness follows the sentence dynamics —
the "quality comes and goes" the owner heard on 2026-09-24.

# Design

`x ── × g(t) ──▶ core ──▶ × 1/g(t − delay) ──▶ out`

- `g` is **attenuation only** (`g ≤ 1`). Boosting would also lift a
  quiet talker across the room into the level window where Hush
  passes a lone voice as the primary speaker; with attenuation only,
  input at or below the target is untouched and behaves exactly as
  the plain stage did in the owner's live acceptance.
- The anchor follows the frame level in dB with a **slew-rate limit**
  in each direction: fast up (`ATTACK_DB_PER_SECOND`) so the talker's
  first loud sentence sets it within a couple of seconds while a
  100 ms clap moves it by only 2 dB; slow down
  (`RELEASE_DB_PER_SECOND`) for frames within `RELEASE_GATE_DB` of
  the anchor, i.e. the same talker getting quieter; and a crawl
  (`LEAK_DB_PER_SECOND`) for anything further below — breaths and
  room tone between sentences, or a quieter second talker — so they
  do not hand the next loud sentence to the core hot. Frames below
  `SPEECH_FLOOR_DBFS` hold the anchor, but not the floor's peak-hold
  below, whose hold and fall are wall time: after a second or two of
  pause the floor has released the trim to 0 dB, and the next
  sentence is trimmed from its own level.
- The trim is **floored on the sentence-scale level** so the talker's
  own quiet sentences never reach the core in the region the sweep
  shows a lone voice being gated or zeroed: a peak-hold of the frame
  level (held for `FLOOR_HOLD_SECONDS`, then falling at
  `FLOOR_DECAY_DB_PER_SECOND`) is never trimmed below
  `CORE_FLOOR_DBFS`. Loud sentences get the anchor's full trim with
  their syllable dynamics intact, sentences already under the floor
  get none, and a sentence in between is lifted onto the floor within
  about a second and a half — at once when a pause of a little over a
  second precedes it.
  (The anchor alone would send a −42 dBFS sentence after a −20 dBFS
  one to the core at −57 dBFS, where the sweep records the output
  zeroed.)
- The gain ramps linearly across each frame (no zipper noise) and the
  per-sample gain is remembered for `delay` samples so the restore
  divides each output sample by the gain that was applied to the
  input it came from: with an identity core the pair is the identity
  to float precision (a unit test pins it).

What the core sees is therefore a compressed copy of the input whose
loud talker sits at the target; what leaves the stage has the input's
dynamics back. Only the core's decisions change.

## leveler-test-scenarios

Source: `crates/noican-models/src/stages/leveler.rs` lines 521–523.

The owner's spread: sentences at −20 dBFS and at −42 dBFS. The
anchor pins to the loud ones; the quiet ones must still reach the
core at or above the floor, and the loud ones at the target.

Source: `crates/noican-models/src/stages/leveler.rs` lines 576–579.

The owner's pauses sit far under the speech floor. The hold and
the fall of the floor's peak-hold run through them, so a quiet
sentence after a loud one and a pause is untrimmed from its first
frame — the first second is not spent in the gated region.

## leveler-tuning

Source: `crates/noican-models/src/stages/leveler.rs` lines 61–69.

Anchor target. Hush passes a lone voice at parity around −37 dBFS
segment RMS (`docs/hush-48k-eval.md`, level sweep: own-voice SI-SDR
peaks at −38…−37, output level +0.4 dB there). The anchor sits on the
loud frames, a few dB above the segment RMS, so the target is set
2 dB above that point; measured with the harness (stand-in voice at
−37 / −30 / −22 dBFS, SIR +12, own-voice SI-SDR in dB): target −37 →
12.4 / 15.2 / 14.4, **−35 → 14.3 / 14.1 / 14.0**, −33 → 14.8 / 12.0 /
12.4. −35 is the flattest, i.e. the most level-independent, and
stays within 0.3 dB of the unleveled stage's best case (14.4).

Source: `crates/noican-models/src/stages/leveler.rs` lines 72–73.

Frames quieter than this do not move the anchor (room tone and
pauses; VCTK stand-in speech frames sit above −55 dBFS at the target).

Source: `crates/noican-models/src/stages/leveler.rs` lines 76–78.

Anchor slew upward. 20 dB/s: a talker 15 dB above the target is
trimmed to it within ≈ 1 s (2.4 s to settle within 1 dB on the
stand-in), while a 100 ms transient moves the anchor by 2 dB.

Source: `crates/noican-models/src/stages/leveler.rs` lines 81–86.

Anchor slew downward for frames within [`RELEASE_GATE_DB`] of the
anchor. 3 dB/s: a within-sentence dip of 10 dB lasting 300 ms
releases the trim by under 1 dB; a talker who genuinely gets quieter
is followed within seconds. Measured against 2 and 10 dB/s on the
stand-in (10 dB/s let the trim breathe with the syllables and left
the core 9 dB hot; 2 dB/s settled in 13 s instead of 2.4 s).

Source: `crates/noican-models/src/stages/leveler.rs` lines 89–98.

Frames more than this far under the anchor release it only at
[`LEAK_DB_PER_SECOND`]. Without the gate, three seconds of breaths
and room tone at −55 dBFS between sentences released 9 dB of trim
and the next loud sentence reached the core at −28 dBFS (stand-in at
−22 dBFS, per-second trace in the pull request); with the gate the
core's per-second level tracks the −37 dBFS reference within 5 dB
(12 and 15 dB measured alike). 12 dB is chosen so a second talker at
the harness's +12 dB SIR sits outside it and cannot pull the trim
down within their turn, while the talker's own softer sentences
(a few dB) are still followed at the release rate.

Source: `crates/noican-models/src/stages/leveler.rs` lines 101–103.

Anchor crawl for frames under the release gate: a talker who steps
20 dB back from the microphone regains full level in 40 s rather
than never.

Source: `crates/noican-models/src/stages/leveler.rs` lines 106–107.

Deepest trim. Input hotter than target + 30 dB (−7 dBFS RMS) is at
the microphone's own clipping point; the trim stops there.

Source: `crates/noican-models/src/stages/leveler.rs` lines 110–119.

The talker's sustained level is not trimmed below this. It is
measured on a peak-hold of the frame level (see
[`FLOOR_DECAY_DB_PER_SECOND`]), which sits about 5 dB above the
segment RMS the sweep in `docs/hush-48k-eval.md` is expressed in, so
−35 here corresponds to ≈ −40 dBFS RMS there: a lone voice at −40
still comes through (own-voice SI-SDR 9.1 dB, level +1.3 dB) while
one at −40 and below is still suppressed as background (−17.7 dB);
at −38 the second flips (−2.1 dB). The floor therefore keeps the
talker's quiet sentences audible without lifting a second talker
into the primary window.

Source: `crates/noican-models/src/stages/leveler.rs` lines 122–132.

How long the peak-hold level the floor is measured on keeps its
value before it starts to fall. The floor must react to a *sentence*
getting quieter, not to the syllable dips inside a loud one, and it
must have fallen by the time the next sentence starts after an
ordinary pause. The two pull in opposite directions and were
measured with the harness on the static stand-in (−22 dBFS row,
own-voice SI-SDR) and the dynamic one (−20 / −42 / −30 dBFS
sentences, quiet-sentence SI-SDR): 1.0 s → 13.6 / 9.1 dB, 0.6 s →
13.4 / 11.1 dB, 0.4 s → 12.5 / 14.1 dB. 0.6 s keeps the static row
within 0.2 dB while a 1 s pause (a sentence boundary) already
leaves 0.4 s of fall before the next sentence.

Source: `crates/noican-models/src/stages/leveler.rs` lines 135–140.

Fall rate of the peak-hold level once the hold has run out. With
the 0.6 s hold, a 1.0 s pause leaves the next quiet sentence 3 dB
under the floor on its first frame and on the floor 0.3 s later;
pauses of 1.1 s and longer hand it over clean. Measured against
20 dB/s (static −22 dBFS row 13.5 vs 13.4 dB; dynamic quiet
sentences 10.8 / 14.8 vs 11.1 / 15.8 dB).

Source: `crates/noican-models/src/stages/leveler.rs` lines 538–540.

Skip the first 1.5 s of every sentence: the floor's
peak-hold (0.6 s hold, then 30 dB/s: 1.3 s for the 22 dB
step) and the anchor's attack are still moving there.

Source: `crates/noican-models/src/stages/leveler.rs` lines 585–589.

1.2 s of digital silence — an ordinary sentence boundary, and
just past the 0.6 s hold + 0.5 s fall the constants give for a
15 dB trim: anchor held, peak-hold fallen. This is what pins
the tuning: with the earlier 1 s hold and 20 dB/s fall the
−42 dBFS frame below is still trimmed 11 dB here.

## microphone-switch-fallback

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 251–255.

A live change rebuilds the transport with the same model and
mode (a brief gap is inherent). If the rebuild itself fails,
the previous device (which was working a moment ago) is
restored by the `startCompleted` failure transition instead of
leaving the session dead.

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 293–298.

A failed live microphone switch must not kill the session:
fall back to the device that was working a moment ago —
the explicit failure-event → restart-effect transition
(one attempt: the fallback start carries no further revert
target). The reason stays visible under the microphone
list.

## peak-hold-rearm-policy

Source: `crates/noican-models/src/stages/leveler.rs` lines 227–238.

Peak-hold of the frame level: rises at once, holds for
`FLOOR_HOLD_SECONDS` after the last frame at its level, then falls
at `FLOOR_DECAY_DB_PER_SECOND` toward the frame level (no further
than the speech floor), re-arming the hold when it lands on it.
A frame merely *near* the peak does not re-arm the hold: that
made the hold full at the end of every real sentence (syllable
jitter sits within a decibel of the peak) and the first second of
the following pause was spent holding the loud level. It runs
through pauses too — the hold and the fall are wall time, not
speech time — so after a pause longer than the hold the next
sentence sets it on its first frame, whatever its level, and the
floor is right for that sentence from the start.

Source: `crates/noican-models/src/stages/leveler.rs` lines 249–252.

Landed on the frame level: this frame *is* the
sustained level, so it re-arms the hold like any other
peak. Otherwise a level trailing off for longer than
the hold would ride down with the hold spent.

Source: `crates/noican-models/src/stages/leveler.rs` lines 615–618.

The other half of the hold: a pause shorter than it (a breath, a
comma) must not release the floor, or every loud sentence's soft
syllables would be lifted onto it and the next onset would reach
the core hot.

Source: `crates/noican-models/src/stages/leveler.rs` lines 623–626.

A steady tone's frames jitter by 1e-5 dB, so how much of the
hold is left after 4 s of it is arbitrary (only frames at or
above the peak re-arm it); a sentence ends on a syllable peak,
so end on one: a frame above the peak re-arms the hold.

Source: `crates/noican-models/src/stages/leveler.rs` lines 641–646.

The same after a level that trails off for longer than the
hold (each frame 0.05 dB under the last, 2 s) and ends on a
syllable peak a few dB above the trailing level. The floor is
then legitimately engaged for the quieter level; the point is
that the peak re-armed the hold, so the pause does not release
it any further.

## stage-interface

Source: `crates/noican-core/src/stage.rs` lines 38–42.

A fixed-frame mono processor at its native sample rate.

Model implementations implement this; [`crate::framed::FramedStage`]
adapts it to the [`Stage`] interface, absorbing rate and frame-size
differences.

## stage-latency-alignment

Source: `crates/noican-cli/src/process.rs` lines 16–21.

Runs `input` through `stage`, compensating the stage's internal buffering
latency so the output is time-aligned with the input.

# Errors

Propagates stage processing failures.

## stage-process-block-contract

Source: `crates/noican-core/src/stage.rs` lines 19–26.

Process one block. `output` must be the same length as `input` and is
fully overwritten. The stage may impose internal latency: the first
samples it emits are zeros until its pipeline fills.

# Errors

Returns [`StageError::BufferLen`] when `output.len() != input.len()`,
or [`StageError::Inference`] when the backend fails.

Source: `crates/noican-core/src/stage.rs` lines 54–59.

Process exactly one frame: `input.len() == output.len() ==`
[`Self::frame_len`].

# Errors

Returns [`StageError::Inference`] when the backend fails.

## stage-threading-model

Source: `crates/noican-core/src/stage.rs` lines 10–14.

A streaming mono audio processor operating at [`ENGINE_SAMPLE_RATE`].

Implementations must be suitable for use on a dedicated inference thread:
`process_block` may allocate internally (it does not run on the audio I/O
callback; see docs/tech-research.md §9), but should avoid unbounded work.

## stereo-channel-averaging

Source: `crates/noican-cli/src/audio.rs` lines 349–352.

Regression guard against the candidate-B stereo bug (left channel
only): the stereo fixture has the tone on the right channel and
silence on the left, so channel averaging must yield half
amplitude while "left only" would yield silence.

## strength-alignment

Source: `crates/noican-core/src/capture.rs` lines 582–584.

Silent stage with a reported latency, standing in for a model:
at 50% intensity the engine output is exactly the half-gain,
delay-compensated dry path.

Source: `crates/noican-core/src/capture.rs` lines 609–616.

End-to-end strength alignment (the PR #15 C5 measurement, applied
to the native-capture path): speech-like audio captured at 16 kHz
and at 44.1 kHz (an integer and a rational ratio), converted by
the input resampler, and processed at 50% intensity
through a silent stage with reported latency must come out as
exactly half the delayed dry signal — cross-correlation measures
zero residual offset, proving the input resampler's delay cannot
disturb the dry-compensation alignment (both taps sit after it).
