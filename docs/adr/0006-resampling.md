# 0006. Resampling and clock-drift correction

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## clock-drift-servo

Source: `crates/noican-core/src/capture.rs`, `InputResampler::set_drift_ppm`.

Applies a drift correction in parts per million (clamped to
±[`MAX_DRIFT_PPM`]; non-finite values read as zero). Positive
values stretch the stream — more output samples per input sample —
which raises the buffered occupancy downstream.

Source: `crates/noican-core/src/capture.rs`, `DriftServo`.

Ring-occupancy servo steering the drift correction: the control half
of the DIY clock-drift compensation (docs/tech-research.md §4.2).

The capture side produces samples on the microphone's clock; the
output side consumes them on the virtual output's clock. Any rate
mismatch shows up as a trend in the number of samples buffered
between the two, so the servo smooths that occupancy and requests a
correction proportional to its distance from the target:

- occupancy above target → the microphone clock runs fast → squeeze
  (negative ppm, fewer output samples per input sample);
- occupancy below target → stretch (positive ppm).

Proportional control leaves a small steady-state offset (the residual
occupancy error that sustains the correction) — bounded by
`drift × horizon`, e.g. 48 samples (1 ms) at 100 ppm — which is
harmless and keeps the loop trivially stable.

Source: `crates/noican-core/src/capture.rs`, `DriftServo::update`.

Feeds one occupancy observation (total samples buffered between
the capture and output clock domains, in engine-rate samples;
expected once per engine block) and returns the drift correction
in ppm to apply via [`InputResampler::set_drift_ppm`].

Source: `crates/noican-core/src/capture.rs`, `tests::servo_cancels_clock_drift_in_a_simulated_session`.

The capture clock delivers (1 + drift) × nominal; the
conversion applies (1 + correction); the output clock
consumes exactly 480 samples per block.

Source: `crates/noican-core/src/capture.rs`, `tests::servo_holds_occupancy_through_the_rational_resampler`.

The same closed loop through the *real* resampler at the rational
44.1 kHz ratio, mirroring the split worker: a producer clock
delivering 441 × (1 + drift) native samples per 10 ms block — in
Bluetooth-like bursts every third block, so the native ring
carries a real residue — the worker draining that ring in
480-sample passes into the resampler and an engine-rate FIFO, a
consumer taking exactly 480 per block, and the servo fed the
occupancy the worker computes (output ring + FIFO + native ring
through `to_engine_samples`, nonzero on the burst blocks). The
occupancy must stay bounded and settle near the target with the
correction canceling the drift — the integer-factor assumption is
gone from every term.

## clock-drift-servo-tuning

Source: `crates/noican-core/src/capture.rs`, `SERVO_HORIZON_SAMPLES`.

First-order correction horizon in samples at the engine rate: an
occupancy error is corrected with a time constant of
`HORIZON / 48000` ≈ 10 s — far slower than Bluetooth burst jitter,
fast enough to absorb worst-case drift long before the rings fill.

Source: `crates/noican-core/src/capture.rs`, `SERVO_SMOOTHING`.

EMA coefficient applied per [`DriftServo::update`] call (one call per
10 ms engine block → ≈0.5 s smoothing window), filtering Bluetooth
burst-delivery noise out of the occupancy signal.

Source: `crates/noican-core/src/capture.rs`, `tests::servo_holds_occupancy_through_the_rational_resampler`.

Proportional residual (drift × horizon ≈ 96 samples at
200 ppm) plus one block of FIFO granularity.

## drift-compensation-servo

Source: `crates/noican-coreaudio/src/macos/split.rs`, `OUTPUT_PRIME_SAMPLES`.

Silence pushed into the output ring before the units start: the
initial cushion absorbing Bluetooth burst jitter, and the drift
servo's occupancy target (50 ms at 48 kHz). This is added latency of
the split path only; the aggregate path is untouched.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `start`.

Prime the output ring at the servo's occupancy target so playback
starts with the jitter cushion in place.

Source: `crates/noican-coreaudio/src/macos/split.rs`, `split_processing_loop`.

Drift servo: total samples buffered between the capture
clock (producer) and the output clock (consumer), in
engine-rate samples (the native ring's occupancy converted
through the nominal ratio). A trend here *is* clock drift.

## drift-correction-design

Source: `crates/noican-core/src/resample.rs`, `MAX_DRIFT_PPM`.

Largest drift correction a [`PolyphaseResampler`] applies, in parts
per million.

±2000 ppm (0.2%) is an order of magnitude above real crystal
mismatches, small enough to be inaudible (≈3.5 cents), and bounds how
fast a servo can slew the ratio while recovering a priming offset.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::set_drift_ppm`.

Applies a drift correction in parts per million (clamped to
±[`MAX_DRIFT_PPM`]; non-finite values read as zero). Positive
values stretch the stream — more output samples per input sample.
At exactly zero the phase step is an integer and the conversion is
the exact rational polyphase result.

Source: `crates/noican-core/src/resample.rs`, `tests::polyphase_tracks_a_fixed_drift_correction`.

A fixed drift correction time-warps the output by exactly that
ratio: the count follows `1 + ppm`, and the waveform tracks the
analytically warped sine as accurately as the undrifted
conversion — including at 15 kHz, where a Catmull-Rom drift stage
at the output rate would already droop measurably. (Measured:
93.6 dB at 1 kHz and 94–97 dB at 15 kHz for both 0 and −500 ppm;
the phase interpolation adds no visible error.)

Source: `crates/noican-core/src/resample.rs`, `tests::polyphase_tracks_a_fixed_drift_correction`.

Output sample n sits at input time n / stretch; the
filter delay is fixed in input time.

## input-resampler-reset

Source: `crates/noican-core/src/capture.rs`, `InputResampler::reset`.

Clears filter history (the drift correction persists).

## integer-group-delay

Source: `crates/noican-core/src/resample.rs`, `taps_for_factor`.

Number of taps used for a given integer factor. Odd so the group delay is
an integer number of samples at the high rate.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::new`.

Group delay `(num_taps - 1) / 2 = step × delay` prototype
samples is exactly `delay` output samples.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::delay_output_samples`.

Group delay in samples at the output rate — an integer by
construction.

Source: `crates/noican-core/src/resample.rs`, `tests::polyphase_reduces_ratios_and_reports_integer_delays`.

The ratio reduces exactly and the integer output delay follows
`num_taps - 1 = 2 · step · delay`.

Source: `crates/noican-core/src/resample.rs`, `tests::polyphase_reduces_ratios_and_reports_integer_delays`.

The integer factors keep the delays of the fixed-factor
designs (`taps_for_factor`: 40 × factor + 1 taps).

Source: `crates/noican-core/src/resample.rs`, `tests::polyphase_reduces_ratios_and_reports_integer_delays`.

44.1 → 48 kHz: 160 phases, step 147, delay ⌈20·160/147⌉ = 22.

## native-rate-capture-and-drift

Source: `crates/noican-core/src/capture.rs`.

Native-rate capture support for non-48 kHz microphones (issue #7).

Microphones that cannot run at the 48 kHz engine rate — Bluetooth
telephony profiles (HFP/SCO at 8/16/24 kHz), 44.1 kHz-family devices
(44.1/22.05/11.025 kHz), high-rate-only interfaces (88.2/96 kHz) —
are captured at their native rate and converted to 48 kHz on the
inference worker. Two problems are solved here, both
platform-independently so they stay unit-tested on every CI target:

1. **Rate conversion** ([`InputResampler`]): any native rate within
   [`MIN_NATIVE_RATE`] to [`MAX_NATIVE_RATE`] (inclusive) is converted by the
   exact reduced ratio through one [`PolyphaseResampler`] — 160/147
   for 44.1 kHz, 3/1 for 16 kHz, 1/2 for 96 kHz. (Earlier revisions
   accepted integer divisors of 48 kHz only; the rational polyphase
   design removed that limit without adding a stage or a dependency —
   see the type's docs for the alternatives weighed.)
2. **Clock drift** ([`DriftServo`] + the fractional phase step of the
   same resampler): with the microphone and the virtual output on
   separate AUHAL instances there is no Aggregate Device to absorb
   the clock split, so drift is compensated by adapting the effective
   conversion ratio a few hundred ppm around the nominal ratio,
   steered by the buffered-sample occupancy between the two clock
   domains (docs/tech-research.md §4.2, the DIY fallback). The servo
   works in engine-rate samples and is ratio-agnostic; the transport
   converts its native-ring occupancy through
   [`InputResampler::to_engine_samples`].

Everything is preallocated at construction time; `process` and
`update` perform no locking and no allocation in steady state (the
inference worker calls them, never the audio I/O callback —
docs/tech-research.md §9).

Alignment note for the strength control: the dry/wet mixer taps its
dry signal from the *engine input*, i.e. after this conversion, so
the input resampler's delay shifts both paths equally and the
dry-compensation alignment (`crate::mix`) is unaffected by
construction. The `strength_alignment_survives_the_input_resampler`
test below proves it end to end.

## phase-grid-resolution

Source: `crates/noican-core/src/resample.rs`, `MIN_PHASES`.

Fewest polyphase branches per input sample a [`PolyphaseResampler`]
builds. A small upsampling factor (×2, ×3) is oversampled to at least
this many phases so that the linear interpolation between adjacent
phase filters — which is what makes the ratio *arbitrary* — happens on
a grid of at least 128 × the input rate. At 128 phases the
interpolation error is that of a linear interpolator running at
≥ 1 MHz: below 0.001 dB of passband droop and below −90 dB of image
leakage for any audio-band tone. Measured (1 s sines, 44.1 → 48 kHz,
`polyphase_tracks_a_fixed_drift_correction`): 93.6 dB SNR at 1 kHz
and 94–97 dB at 15 kHz for both 0 ppm and −500 ppm — the phase
interpolation adds no error visible above the filter's own. The cost
of more phases is memory only (~40 taps × phases × 4 bytes).

## phase-position-rounding

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::locate`.

Splits `pos` into the newest input sample the next output uses
(`base`, an index into `buf`) and the fractional phase position in
`0.0..num_phases`. Integer positions (zero drift) split exactly;
the guards absorb a floor landing one off from f64 rounding.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::locate`.

f64 rounding can land the floor one off; the true position is
never more than one phase-count away, so one correction suffices.

## polyphase-resampler-design

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler`.

Streaming arbitrary-ratio polyphase FIR resampler.

Exact rational conversion between two sample rates, with a fractional
phase step that folds a parts-per-million drift correction into the
same stage.

# Design (the capture-path resampler decision)

Three ways to accept 44.1 kHz-family microphones on the split
transport were weighed:

- **Rational `L/M` stage in front of the existing cubic drift
  stage.** Minimal change, but two cascaded interpolators, and the
  Catmull-Rom drift stage — fine for telephony content far below
  Nyquist — droops ≈ 8 dB at 20 kHz at half-sample phase, which a
  full-band 44.1 kHz microphone would hear as slow amplitude
  modulation of its top octave as the drift phase cycles.
- **`rubato`** (already a CLI dependency for offline conversion).
  Its asynchronous sinc resampler is the same algorithm as this type,
  but it would pull the crate and its FFT stack into the engine
  dylib, its fixed-chunk API needs a staging layer, and its delay
  reporting is not integer-exact at the output rate.
- **This type**: one windowed-sinc polyphase bank sampled at
  `num_phases` positions per input sample (the exact `L` of the
  reduced ratio, oversampled to at least 128 phases), read at a
  fractional phase step. At zero drift the step is an integer, so the
  output is the *exact* rational polyphase result (no interpolation
  at all: 160/147 for 44.1 kHz, 3/1 for 16 kHz, 1/2 for 96 kHz). At
  nonzero drift, adjacent phase filters are linearly interpolated,
  which on a ≥ 128× grid is far more accurate than a cubic at the
  output rate. Integer factors, the 44.1 kHz family, and downsampling
  from 88.2/96 kHz are all the same code path; the anti-imaging
  filter and the drift stage are one filter, so there is nothing to
  cascade, and the group delay is an integer number of output samples
  by construction (`num_taps - 1 = 2 · step · delay`).

The third option supersedes the first two — no extra dependency, one
stage instead of two, better drift-stage fidelity — so it is the one
implemented.

# Real-time properties

Everything is allocated in [`PolyphaseResampler::new`]. `process`
performs no allocation while each input chunk stays within
`max_input_len` (the caller's output `Vec` must be reserved for
[`PolyphaseResampler::max_output_len`] samples), holds no locks, and
costs two `phase_len`-tap dot products per output sample (one when
the drift correction is zero).

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::new`.

The prototype runs at `num_phases × input_rate`; its passband
must stop below the lower of the two Nyquist frequencies:
1 / (2 · num_phases) (input) or 1 / (2 · step) (output).

## polyphase-resampler-invariants

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::pos`.

Position of the next output sample in phase units relative to
`buf[0]`; always ≥ `(phase_len - 1) × num_phases` so every tap
has a sample.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::new`.

Retained between calls: the FIR look-behind plus the one
look-ahead sample the phase interpolation may need.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::max_output_len`.

Plus the look-ahead sample released from the previous call and
one for the ceiling of the fractional phase.

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::process`.

Drop the consumed prefix, keeping the FIR look-behind of the
next output sample (which may lie past the buffered input when
downsampling — clamp to the buffer).

## polyphase-streaming-contract

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::process`.

Consumes `input`, appending every output sample whose filter
support is complete to `output` (one input sample of look-ahead
is held back until the next call; the signal itself is delayed by
exactly [`PolyphaseResampler::delay_output_samples`]).

## resampler-buffer-sizing

Source: `crates/noican-core/src/capture.rs`, `InputResampler::max_output_len`.

Upper bound on the samples one [`InputResampler::process`] call
appends for `input_len` native samples (at the largest drift
correction); reserve output buffers to it.

## resampler-input-contracts

Source: `crates/noican-core/src/resample.rs`, `Decimator`.

Streaming decimator by an integer factor (input at the high rate, output
at the low rate). Inputs must be supplied in multiples of the factor.

## resampler-ratio-units

Source: `crates/noican-core/src/capture.rs`, `InputResampler::ratio`.

Reduced conversion ratio `(up, down)`: `up` engine samples per
`down` native samples at zero drift (160/147 for 44.1 kHz, 3/1 for
16 kHz).

Source: `crates/noican-core/src/capture.rs`, `InputResampler::to_engine_samples`.

Converts a count of native-rate samples to the engine-rate samples
they will become (rounded to nearest), so a transport can express
its native-ring occupancy in the servo's units.

## resampler-reset-semantics

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::reset`.

Clears filter history (the drift correction persists).

## resampling-architecture

Source: `crates/noican-core/src/resample.rs`.

Streaming polyphase FIR resampling.

Two families share one Kaiser-windowed sinc design:

- [`Decimator`] / [`Interpolator`]: integer factors for the model path
  (the engine runs at 48 kHz; some models run at 16 kHz, 48000 / 16000
  = 3). Fixed, tiny, deterministic.
- [`PolyphaseResampler`]: arbitrary ratios for the capture path — the
  exact rational `L/M` between a microphone's native rate and the
  engine rate (160/147 for 44.1 kHz, 3/1 for 16 kHz, 1/2 for 96 kHz),
  plus a fractional phase step that folds in the clock-drift
  correction, so one stage does both jobs.

Everything is allocation-free after construction (safe for a
real-time inference thread).

## shared-filter-span

Source: `crates/noican-core/src/resample.rs`, `HALF_SPAN_LOW_RATE`.

Half the filter span in samples at the *lower* of the two rates: the
same 40-sample windowed-sinc span the integer-factor designs above
use (`taps_for_factor`), which the model path has validated at
> 50 dB round-trip SNR.

Source: `crates/noican-core/src/resample.rs`, `tests::polyphase_upsampling_keeps_the_top_of_the_band`.

Upsampling 44.1 → 48 kHz keeps the passband flat well into the top
octave: a 15 kHz sine (0.68 × the input Nyquist) comes out above
60 dB SNR. (Measured on this design: ≈94 dB at 15 kHz, 63 dB at
17 kHz, then the 40-sample-span transition band, −4 dB at 19 kHz.
The roll-off is the shared `HALF_SPAN_LOW_RATE` design, identical
to the model path's 16 kHz decimator relative to its Nyquist.)

## untrusted-rate-bounds

Source: `crates/noican-core/src/resample.rs`, `PolyphaseResampler::new`.

Creates a converter from `input_rate` to `output_rate` Hz,
preallocating for input chunks of up to `max_input_len` samples
per [`PolyphaseResampler::process`] call.

The bank has `up × oversampling` phases of `~40 × max(1, down/up)`
taps, where `up/down` is the reduced ratio: callers exposing this
to untrusted rates should bound the terms of
[`PolyphaseResampler::reduced_ratio`] first (a rate coprime with
the other reduces to tens of thousands of phases).

# Panics

Panics when either rate is zero.
