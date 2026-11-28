# 0010. Evaluation methodology

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## audio-file-formats

Source: `crates/noican-cli/src/audio.rs`.

Audio file reading/writing at the engine rate.

Input files of any rate/channel count are converted to mono 48 kHz f32
(channel average + FFT resampling). WAV is read with `hound` (the
long-verified path); AIFF/AIFC, CAF, FLAC, and M4A (AAC/ALAC) are read
with `symphonia`. Output is always 16-bit WAV. This is the offline path;
quality matters more than allocation behavior here.

Some compressed AIFC variants (e.g. IMA4) are not decodable here;
convert those with macOS's built-in
`afconvert -f WAVE -d LEI16@48000 <in> out.wav` first.

## audio-format-test-tolerances

Source: `crates/noican-cli/src/audio.rs`, `tests::m4a_aac_decodes_the_tone`.

AAC is lossy (edge ringing changes the peak) and carries encoder
delay; allow padding differences and search a small lag window.

## audio-test-fixtures

Source: `crates/noican-cli/src/audio.rs`, `tests::FIXTURE_LEN`.

All fixtures encode the same 0.25 s, 440 Hz, -6 dBFS sine at 48 kHz.

## batch-output-layout

Source: `crates/noican-cli/src/process.rs`, `process_file`.

Processes `input_path` through every model in `model_ids`, writing
`out_dir/<input_stem>/<model_id>.wav` plus a `reference.wav` (the input
converted to mono 48 kHz) for fair comparison.

`make_stage` maps a model id to a fresh stage (state is never shared
between files).

# Errors

Fails on I/O errors or when a stage cannot be created/run.

## blind-listening-set

Source: `crates/noican-cli/src/eval_run.rs`, `EvalRequest::seed`.

Seed for lettering the blind listening set.

## cli-file-mode-model-comparison

Source: `crates/noican-cli/src/main.rs`.

noican CLI: model weight management and batch WAV processing.

The same engine stages used by the real-time pipeline are driven here in
file mode, giving strictly identical conditions for model comparison
(docs/tech-research.md §12, Phase 0).

Source: `crates/noican-cli/src/main.rs`, `Command::Process`.

Process audio files through models; outputs are organized per input
file for side-by-side comparison.

## cli-supported-audio-formats

Source: `crates/noican-cli/src/main.rs`, `Command::Process`.

Input audio files (WAV, AIFF/AIFC, CAF, FLAC, M4A; output is WAV).

## deterministic-noise-input

Source: `crates/noican-models/examples/block_bench.rs`, `main`.

Deterministic noise input: block cost is content-independent for
these architectures, and noise avoids shipping an audio fixture.

## deterministic-test-signal

Source: `crates/noican-models/tests/model_integration.rs`, `test_signal`.

xorshift32; deterministic, no external RNG dependency.

## eval-band-energy

Source: `crates/noican-cli/src/eval.rs`, `BAND_FFT_LEN`.

Analysis window of the band-energy estimate: 1024 samples at 48 kHz
gives 46.9 Hz bins, fine enough that the 8 kHz edge falls within one
bin of the nominal value.

Source: `crates/noican-cli/src/eval.rs`, `band_energy`.

Energy of `samples` between `lo_hz` (inclusive) and `hi_hz`
(exclusive), summed over Hann-windowed [`BAND_FFT_LEN`] frames at half
overlap. The scale is arbitrary but identical for equal-length inputs,
which is all the ratios here need. Returns 0 for inputs shorter than
one frame.

## eval-blind-listening-order

Source: `crates/noican-cli/src/eval.rs`, `blind_order`.

A deterministic permutation of `0..n` from `seed` (Fisher–Yates over
a splitmix64 stream), used to letter the blind listening set so the
owner cannot infer a model from file order.

## eval-high-band-edge

Source: `crates/noican-cli/src/eval.rs`, `HIGH_BAND_HZ`.

Lower edge of the "high band" whose survival distinguishes a 48 kHz
path from a 16 kHz one. 8 kHz is the 16 kHz model's Nyquist frequency.

## eval-input-semantics

Source: `crates/noican-cli/src/eval_run.rs`, `EvalRequest::targets`.

Clean recording(s) of the user's own voice, concatenated in order.

Source: `crates/noican-cli/src/eval_run.rs`, `EvalRequest::interferers`.

Recording(s) of the interfering speaker, concatenated in order.

Source: `crates/noican-cli/src/eval_run.rs`, `EvalRequest::sirs_db`.

Signal-to-interference ratios (dB) of the overlap segment.

Source: `crates/noican-cli/src/eval_run.rs`, `EvalRequest::target_level_dbfs`.

RMS level (dBFS) to normalize the voice to; `None` keeps the
recorded level.

Source: `crates/noican-cli/src/eval_run.rs`, `EvalRequest::segment_seconds`.

Requested segment length in seconds (clamped to the material).

## eval-metric-definitions

Source: `crates/noican-cli/src/eval.rs`.

Objective metrics for comparing speaker-suppression candidates.

The question this module answers is the one behind the re-scoped
Phase 1 item (docs/tech-research.md §6.4): *does a candidate keep
Hush's background-speaker suppression while keeping the user's own
voice at 48 kHz bandwidth?* Listening decides in the end, but the
candidates are first ranked on numbers that come out of a controlled
mixture:

```text
|---- you only ----|---- you + other (at SIR) ----|---- other only ----|
0                  L                              2L                   3L
```

`L` is one segment ([`build_mixture`]); the clean own-voice recording
supplies the first two segments, the interferer recording supplies the
last two, scaled so that the RMS ratio inside the middle segment is the
requested signal-to-interference ratio. The processed output is
time-aligned with the mixture (the CLI compensates each stage's
reported latency), so every metric is a plain comparison over one
region, skipping [`REGION_GUARD`] at each region start so a model's
adaptation to the new condition does not count against it:

- **High-band retention** (you only): energy at or above
  [`HIGH_BAND_HZ`] in the output relative to the clean voice, in dB.
  A 16 kHz model scores about −80 dB here (the resampler's stopband;
  −83.6 dB measured for `hush`); a transparent 48 kHz path scores 0 dB.
- **Own-voice level** (you only): output RMS relative to the clean
  voice, the loudness-parity figure `HUSH_MAKEUP_GAIN_DB` was set
  from (±1 dB is the acceptance band there).
- **Own-voice SI-SDR** (you only): scale-invariant SDR of the output
  against the clean voice. Artefacts, band limits and pumping all
  lower it; the metric is scale-invariant on purpose because gain
  parity is the previous figure.
- **Both-talking SI-SDR** (you + other): the same against the clean
  voice while the interferer is present. The mixture itself scores
  about the SIR; a suppressor should score above it.
- **Interferer residual** (other only): output RMS relative to the
  mixture RMS, full band and at or above [`HIGH_BAND_HZ`]. More
  negative is better. The high-band figure is the leak a band-split
  design would show if its upper band were not gated.

Everything here is pure arithmetic on `f32` slices so it is unit-tested
against synthetic signals with known answers; file I/O and stage
execution live in `main.rs` / `process.rs`.

## eval-metric-thresholds

Source: `crates/noican-cli/src/eval_run.rs`, `HIGH_BAND_SHARE_WARN_DB`.

Below this high-band share a recording cannot exercise the high-band
metrics (a 16 kHz capture such as Bluetooth HFP sits near −100 dB).

Source: `crates/noican-cli/src/eval_run.rs`, `MIN_SEGMENT`.

Shortest segment the metrics are meaningful for (one second: the
region guard plus enough speech for a stable RMS).

## eval-mixture-layout

Source: `crates/noican-cli/src/eval.rs`, `Mixture::target`.

The clean voice, zero outside its two segments (the interferer
component is `input - target`).

Source: `crates/noican-cli/src/eval.rs`, `build_mixture`.

Lays out the three-segment mixture described in the module docs.

`segment_len` is `L` in samples; `target` must provide at least `2L`
samples and `interferer` at least `2L`. The interferer gain is set on
the two halves that actually overlap (target `L..2L` against
interferer `0..L`), so the SIR inside the middle region is exact.

# Errors

Returns a message when either recording is too short or `segment_len`
is zero.

## eval-output-alignment

Source: `crates/noican-cli/src/eval.rs`, `evaluate`.

Computes [`Metrics`] for `output`, which must be time-aligned with
`mixture.input` (same length, latency already compensated).

## eval-progress-callback

Source: `crates/noican-cli/src/eval_run.rs`, `run`.

Runs the evaluation. `make_stage` maps a model id to a fresh stage;
`progress` receives human-readable lines (the table is emitted through
it too, so the binary decides where output goes).

# Errors

Fails on unreadable material, material too short for one second per
segment, stage construction/processing errors, or I/O errors.

## eval-region-guard

Source: `crates/noican-cli/src/eval.rs`, `REGION_GUARD`.

Samples skipped at the start of every region before measuring, so a
model's reaction time to a new condition (Hush's ERB/DF stages
switching on the local SNR estimate, smoothing filters settling) is
not scored as leakage or damage. 250 ms at the engine rate.

## eval-run-contract

Source: `crates/noican-cli/src/eval_run.rs`, `tests::run_writes_mixtures_rows_and_a_consistent_blind_key`.

The whole command path with synthetic material and a passthrough
stage: files land where the docs say, one row per (model, SIR),
and every `blind-key.txt` line names a file whose samples equal
the model output it claims to be.

## eval-si-sdr

Source: `crates/noican-cli/src/eval.rs`, `si_sdr_db`.

Scale-invariant signal-to-distortion ratio of `estimate` against
`reference`, in dB (Le Roux et al., 2019). The estimate is projected
onto the reference, so a pure gain change scores the ceiling
([`SI_SDR_CEILING_DB`]); a silent estimate scores [`DB_FLOOR`].

## eval-silence-trimming

Source: `crates/noican-cli/src/eval.rs`, `TRIM_FRAME`.

Frame used by [`trim_silence`]: 10 ms at the engine rate, matching
the worker block so "silence" means the same thing as in the live path.

Source: `crates/noican-cli/src/eval.rs`, `TRIM_THRESHOLD_DBFS`.

Frames quieter than this (RMS, dBFS) at the head and tail of a
recording are dropped by [`trim_silence`]. −50 dBFS sits well below
speech recorded at conversational level on a built-in microphone
(−36 dBFS RMS in the 2026-09-11 acceptance record) and well above the
noise floor of the recordings this tool is meant for.

Source: `crates/noican-cli/src/eval.rs`, `trim_silence`.

Drops leading and trailing 10 ms frames whose RMS is below
[`TRIM_THRESHOLD_DBFS`]. Internal pauses are kept (they are part of
natural speech and of what the models see live).

Source: `crates/noican-cli/src/eval.rs`, `trim_silence`.

`first` is loud, so the reverse search always finds a frame.

## exact-integer-float-conversion

Source: `crates/noican-core/src/resample.rs`, `ToF64`.

Exact `usize → f64` for the small integers (buffer indices, phase
counts) this module handles.

## hush-eval-methodology

Source: `crates/noican-cli/src/main.rs`, `Command::Eval`.

Score speaker-suppression candidates on synthetic mixtures of your
clean voice and an interfering speaker: high-band retention,
own-voice SI-SDR, interferer residual (full band and ≥ 8 kHz),
latency and block time per model and SIR. Also writes a blind
listening set (docs/hush-48k-eval.md).

Source: `crates/noican-cli/src/main.rs`, `Command::Eval`.

Clean recording(s) of your own voice (48 kHz); several files
are trimmed and concatenated in order, and the first two
segments are taken from the result.

Source: `crates/noican-cli/src/main.rs`, `Command::Eval`.

Recording(s) of the interfering speaker; several files are
trimmed and concatenated in order.

Source: `crates/noican-cli/src/main.rs`, `Command::Eval`.

Signal-to-interference ratios in dB for the overlap segment
(voice RMS over interferer RMS).

Source: `crates/noican-cli/src/main.rs`, `Command::Eval`.

Normalize the voice recording to this RMS level (dBFS) before
mixing. Default: keep the recorded level. Hush's behavior
depends on the absolute input level, so compare candidates at
the level your microphone actually delivers (see
docs/hush-48k-eval.md).

Source: `crates/noican-cli/src/main.rs`, `Command::Eval`.

Segment length in seconds; each mixture is three segments
(you / you + other / other). Clamped to the material.

## input-stem-collision

Source: `crates/noican-cli/src/process.rs`, `ensure_distinct_outputs`.

Decided 2026-11-28; recorded here because code comments are banned.

`noican process` names each output directory after the input's file
stem (see `batch-output-layout`), so `a/take.wav` and `b/take.wav`,
or `take.wav` and `take.flac`, wrote into one directory and the
later input replaced the earlier one's results without a word. The
command now refuses such a list before it processes anything, naming
both inputs. Stems are compared without regard to case, because the
default macOS volume would merge `Take` and `take` as well. Renaming
the colliding directories automatically was rejected: the layout is
documented, and a name that depends on what else was on the command
line cannot be predicted by the scripts that read the output. This
mirrors `sir-dirname-collision`.

## integration-test-tolerances

Source: `crates/noican-models/tests/model_integration.rs`, `run_model`.

Denoisers may attenuate the synthetic signal heavily, but the
pipeline must not explode.

## offline-live-block-parity

Source: `crates/noican-cli/src/process.rs`, `BLOCK_LEN`.

Block size (48 kHz samples) used to drive stages. Mirrors a realistic
real-time block so offline results match live behavior.

## sir-dirname-collision

Source: `crates/noican-cli/src/eval_run.rs`, `validate_sirs`.

Rejects SIR lists whose directory names collide (the name rounds to
whole dB, so `6` and `6.4` would silently overwrite each other's
mixtures and blind set) and non-finite values.

## synthetic-test-material

Source: `crates/noican-cli/src/eval_run.rs`, `tests::synthetic`.

A tone plus a little deterministic noise so trimming
and the band metrics have something to work with.

## test-signal-precision

Source: `crates/noican-core/src/resample.rs`, `tests::sine`.

Amplitude-0.5 sine, phase accumulated in f64 so long high-frequency
test signals (e.g. 30 kHz at 96 kHz for a second) stay exact.
