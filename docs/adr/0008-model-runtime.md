# 0008. Model runtime (ONNX / DeepFilterNet)

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## deepfilternet-embedded-weights

Source: `crates/noican-models/src/manifest.rs`, `ALL_MODELS`.

Embedded in the deep_filter crate (default-model feature).

## deepfilternet-tract-runtime

Source: `crates/noican-models/src/stages/dfn_tract.rs`.

DeepFilterNet-family stages via the upstream `deep_filter` (tract) crate.

DeepFilterNet3 and Hush ship their networks as three ONNX graphs
(`enc`/`erb_dec`/`df_dec`) with a free time axis and *implicit*
recurrent state, which ONNX Runtime cannot stream one hop at a time.
The upstream `deep_filter` crate solves this with tract-pulse (the same
route Hush's own `libweya_nc` takes), so these two models run on tract
while every other model runs on ONNX Runtime — hidden behind the same
[`FrameProcessor`] interface.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `magnitude_ratio`.

Ratio of enhanced to noisy magnitude from the two band energies: the
broadband gain the model applied there, `sqrt(Σ|Y|² / Σ|X|²)`,
clamped to `0..=1` (the network's ERB mask and deep-filter stage
attenuate; any apparent amplification is numerical). A zero (or
denormal) noisy energy yields 0.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::params`.

Kept to rebuild the model on reset (`DfTract` has no state-reset API).

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::deepfilternet3`.

DeepFilterNet3 48 kHz baseline from the model bundle embedded in the
`deep_filter` crate (no download required).

# Errors

Returns [`StageError::Inference`] when tract fails to build the
model.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::deepfilternet3_ll`.

DeepFilterNet3's zero-lookahead 48 kHz variant from its ONNX tarball
(`DeepFilterNet3_ll_onnx.tar.gz`: `df_lookahead = conv_lookahead =
0`), with the same runtime thresholds as [`Self::deepfilternet3`].

# Errors

Returns [`StageError::Inference`] when the tarball cannot be loaded.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::hush`.

Hush 16 kHz from its ONNX tarball (`advanced_dfnet16k_*.tar.gz`),
using the thresholds Hush's own `weya_nc` runtime uses
(min −15 dB, ERB/DF max 35 dB).

# Errors

Returns [`StageError::Inference`] when the tarball cannot be loaded.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::band_gain`.

Broadband gain the model applied to the last processed frame over
`bins` (STFT bin indices, `bin × sample_rate / fft_size` Hz),
`sqrt(Σ|Y|² / Σ|X|²)` of the enhanced over the noisy spectrum,
clamped to `0..=1`.

The upstream `DfTract` exposes both spectra publicly
(`get_spec_noisy` / `get_spec_enh`); with Hush's zero lookahead
they describe the same frame, after the ERB mask, the deep-filter
stage and the attenuation limit — i.e. exactly what the time
domain output was synthesised from. A frame the model skipped as
silent reports 0 (its output was zeroed).

# Panics

Panics when `bins` exceeds [`Self::n_freqs`].

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::from_params`.

Overlap-add window delay plus the model's lookahead frames.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::reset`.

DfTract exposes no state reset; rebuild the plans. This is a
control-plane operation (model switch / stream restart), not an
audio-thread one, so the cost is acceptable.

## dfn3-ll-latency

Source: `crates/noican-models/tests/model_integration.rs`, `dfn3_ll_latency_is_twenty_ms`.

The zero-lookahead variant's delay is one hop of framing plus the
overlap-add window: 960 samples (20 ms) at 48 kHz, half of `dfn3`'s.

## dpdfnet-lookahead

Source: `crates/noican-models/src/stages/dpdfnet.rs`, `LOOKAHEAD_HOPS`.

Model lookahead in STFT hops, on top of the overlap-add window delay.

The DPDFNet architecture (`DeepFilterNet2` + dual-path RNN) keeps
DFN2's two hops of convolutional lookahead plus two hops of
deep-filtering lookahead. The streaming ONNX exports carry no
latency/lookahead metadata key, so this is a constant of the
architecture, confirmed empirically: cross-correlating real speech
against the latency-aligned CLI output measured **exactly 4 hops
(1920 samples, 40 ms) of unreported delay on both the
`dpdfnet2_48khz_hr` and `dpdfnet8_48khz_hr` profiles** (2026-08-27,
`x86_64` ONNX Runtime). Without it, the intensity control's
delay-compensated dry path is 40 ms early and a partial mix doubles
the voice (hardware test C5).

## dpdfnet-stft-pipeline

Source: `crates/noican-models/src/stages/dpdfnet.rs`.

DPDFNet 48 kHz HR streaming stage.

The graph consumes one raw complex STFT frame (`spec [1, 1, bins, 2]`)
and returns the enhanced frame plus an opaque flat state vector; ERB
features, normalization, masking, and deep filtering all live inside
the graph. Outside the graph (mirroring sherpa-onnx's
`online-speech-denoiser-stft-impl.h` and ceva-ip's `stream.py`):
sliding analysis window → analysis window function → unnormalized rfft
→ run graph → irfft (scaled 1/n) → synthesis window (same window) →
50% overlap-add, emitting `hop` samples per call. The window satisfies
squared-COLA, so no OLA normalization is needed. All STFT parameters
and the state-vector initialization come from the ONNX metadata.

Source: `crates/noican-models/src/stages/dpdfnet.rs`, `DpdfnetStage::new`.

Loads a DPDFNet streaming ONNX at `path`, reading all STFT and
state parameters from its embedded metadata.

# Errors

Returns [`StageError::Inference`] on load failure, missing
metadata, or an unsupported window type.

## embedded-dfn3-model

Source: `crates/noican-models/tests/model_integration.rs`, `dfn3_runs`.

DeepFilterNet3 is embedded in the binary — runnable even without a
models directory.

## model-declared-size-caps

Source: `crates/noican-models/src/onnx.rs`, `MAX_STATE_ELEMENTS`.

Largest element count accepted for one recurrent state tensor. The
shipped models' state tensors are orders of magnitude smaller; the
cap exists because the shape is model-declared data — multiplying
its dims unchecked would let a crafted model (e.g. dims
`[1, 65536, 65536]`) force a multi-GB zeroing allocation, and an
allocation failure aborts the process rather than unwinding, so the
FFI layer's `catch_unwind` could not contain it. Stage constructors
that size allocations from other model-declared counts (DPDFNet's
`state_size` metadata) bound them by the same constant.

## model-metadata-bounds

Source: `crates/noican-models/src/stages/dpdfnet.rs`, `MAX_WINDOW_LEN`.

Largest plausible STFT window length in samples (the real DPDFNet
profiles use 960–4096). Bounds the model-declared `window_length`
metadata before it sizes buffers and the FFT plan.

Source: `crates/noican-models/src/stages/dpdfnet.rs`, `DpdfnetStage::new`.

win_len sizes the analysis/overlap buffers and the FFT plan;
like every model-declared count it is bounded before it is
trusted. Real profiles use at most a few thousand samples.

Source: `crates/noican-models/src/stages/dpdfnet.rs`, `DpdfnetStage::new`.

hop sizes the framed stage's buffers and indexes the sliding
window: zero would stall the frame loop, and anything above
win_len underflows the delay arithmetic and panics the window
slide. Real profiles use hop = win_len / 2.

Source: `crates/noican-models/src/stages/dpdfnet.rs`, `DpdfnetStage::new`.

state_size sizes the zeroed state vectors below; bound it
like the state-tensor shapes in onnx.rs.

## onnx-runtime-environment-singleton

Source: `crates/noican-models/src/onnx.rs`, `environment`.

The process-wide ONNX Runtime environment. `ort` allows exactly one per
process and every session must be built against it.

## onnx-single-threaded-streaming

Source: `crates/noican-models/src/onnx.rs`, `load_streaming_session`.

Loads an ONNX session tuned for low-latency streaming inference on CPU:
single-threaded (frames are small; thread wakeups cost more than they
save) with full graph optimization.

Single-threading is a measured decision, not a guess. A/B on
2026-09-02 (x86-64, 4 cores, `examples/block_bench.rs`, 6000 blocks
of the heaviest ONNX stage then registered;
docs/acceptance/2026-09-02-underrun-baseline.md):
p50 4.16 ms / p95 6.23 ms with 1 intra-op thread, 6.30 / 9.53 ms with
2 threads, and 5.77 / 8.72 ms with 4 — synchronization overhead
dominates these small per-frame ops, so extra threads make every
percentile worse. Inter-op stays at 1 as well: the graphs are
sequential, so inter-op parallelism only adds scheduling overhead.

# Errors

Returns [`StageError::Inference`] when the file cannot be loaded.

## ulunas-stft-contract

Source: `crates/noican-models/src/stages/ulunas.rs`.

UL-UNAS 16 kHz streaming stage.

The graph consumes one complex STFT frame (`mix [1, 257, 1, 2]`) and
returns the enhanced complex frame plus three opaque cache blobs. The
STFT/iSTFT is computed outside the graph (verified against the repo's
`ulunas_stream.py`, which reproduces the shipped enhanced sample to
1.3e-4): `n_fft` = 512, hop = 256, periodic Hann analysis and synthesis
windows, overlap-add divided by the overlapped squared-window sum.

## upstream-deep-filter-coupling

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `SILENT_FRAME_MEAN_SQUARE`.

Mean-square level below which `DfTract::process` skips the network,
zeroes its output and leaves its spectrum buffers stale (upstream
`tract.rs`, `process`: `if rms < 1e-7 { enh.fill(0.); return … }` —
the variable is named `rms` upstream but holds the mean square).
Mirrored here so [`DfTractStage::band_gain`] can report 0 for such a
frame instead of reading the previous frame's spectra.

Pinned against `deep_filter` **v0.5.6** (the workspace `Cargo.toml`
git tag). This is a silent coupling — no test can compare it with
upstream — so re-read `DfTract::process` when that tag moves.

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `mean_square`.

Mean square of a frame (the quantity upstream compares against
[`SILENT_FRAME_MEAN_SQUARE`]).

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `DfTractStage::last_frame_silent`.

True when the last frame fell under [`SILENT_FRAME_MEAN_SQUARE`]
(the model produced zeros without touching its spectra).
