# Model Weights: Sources, Fetching, and Verification

Model weights are **never committed to this repository**. They are fetched
from the official distribution points below into the `models/` directory
(git-ignored) and verified against pinned SHA-256 digests.

```sh
# List models and their fetch status
cargo run -p noican-cli --release -- models

# Download everything
cargo run -p noican-cli --release -- fetch

# Download specific models
cargo run -p noican-cli --release -- fetch fastenhancer-t dpdfnet2
```

Every source is public. Downloads from huggingface.co send
`NOICAN_HF_TOKEN` (or `HF_TOKEN`) as a bearer token when set, which
raises Hugging Face's rate limits; no token is required.

## Registry

| id | model | family | rate | backend | weights source | license |
|---|---|---|---|---|---|---|
| `fastenhancer-t/b/s/m/l` | FastEnhancer (ICASSP 2026) | denoise | 48 k | ONNX Runtime | [GitHub release `onnx-48khz-v1`](https://github.com/aask1357/fastenhancer/releases/tag/onnx-48khz-v1) | MIT |
| `dpdfnet2` | DPDFNet2 48 kHz HR | denoise | 48 k | ONNX Runtime | [sherpa-onnx release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speech-enhancement-models) | Apache-2.0 |
| `dpdfnet8` | DPDFNet8 48 kHz HR | denoise | 48 k | ONNX Runtime | [HF Ceva-IP/DPDFNet](https://huggingface.co/Ceva-IP/DPDFNet) (not on the sherpa release yet) | Apache-2.0 |
| `dfn3` | DeepFilterNet3 | denoise (baseline) | 48 k | tract (embedded in the `deep_filter` crate) | — (no download) | MIT OR Apache-2.0 |
| `ul-unas` | UL-UNAS (TASLP 2026) | denoise (low-latency) | 16 k | ONNX Runtime | [commit-pinned repo file](https://github.com/Xiaobin-Rong/ul-unas/tree/main/ulunas_onnx/onnx_models) | MIT |
| `hush` | Hush (Weya AI) | speaker suppression | 16 k | tract (`deep_filter` crate, Hush tarball) | [HF weya-ai/hush](https://huggingface.co/weya-ai/hush) | Apache-2.0 |
| `hush-48k` | Hush 48k (band-split wrapper around `hush`) | speaker suppression | 48 k out (16 k core) | tract + `noican-models::stages::hush_wideband` | — (depends on `hush`; no files of its own) | Apache-2.0 |

Sample-rate/frame-size differences are absorbed by the engine
(`noican-core::FramedStage`): 16 kHz models are driven through a
fixed-ratio polyphase resampler and all models present the same 48 kHz
streaming interface.

### Composite entries (`depends_on`)

A registry entry may run another entry's weights instead of shipping its
own (`ModelSpec::depends_on`). `noican fetch <id>` then fetches the
dependency first, `noican models` reports the entry as fetched only when
its dependency is, and the weights live in the dependency's directory
only. `hush-48k` is such an entry: it wraps the 16 kHz Hush core in a
48 kHz band-split stage — the input's band above 8 kHz is added back to
Hush's output, scaled by the gain Hush applied in 4–7 kHz, so the added
band is muted whenever Hush mutes. Same latency as `hush` (1080
samples, 22.5 ms), ≈ 0.01 ms added per 10 ms block; design record in
the module documentation of `crates/noican-models/src/stages/hush_wideband.rs`
and in [tech-research.md §6.4](tech-research.md).

## Batch comparison (CLI file mode)

```sh
# All fetched models, one output directory per input file
cargo run -p noican-cli --release -- process my_recording.wav --out-dir out

# Specific models; comma-separated or repeated
cargo run -p noican-cli --release -- process my_recording.wav --models fastenhancer-s,dpdfnet2
```

Outputs land in `out/<input-stem>/<model-id>.wav` (mono 48 kHz, 16-bit)
next to `reference.wav` (the input converted to mono 48 kHz), so files are
directly comparable in any editor/player. Inputs of any rate/channel count
are accepted; processing runs in realtime-sized blocks through the same
stage code the live engine uses, and each stage's buffering latency is
compensated so outputs are time-aligned with the reference.

## Speaker-suppression evaluation (`noican eval`)

`noican eval` mixes a clean recording of your own voice with a public
interfering-speaker recording at several SIRs, runs the candidate models
under the same conditions as `process`, and reports high-band retention,
own-voice SI-SDR, interferer residual (full band and ≥ 8 kHz), latency
and block time per model — plus a blind listening set. Procedure,
metric definitions and material licensing:
[hush-48k-eval.md](hush-48k-eval.md).

## Verification status (2026-08-25, Linux x86_64)

- All denoise models produce finite, time-aligned, plausibly denoised
  output from a real noisy-speech sample.
- `dpdfnet2`: Rust output correlates **0.998 at lag 0** with an
  independent Python reference implementation of the same pipeline.
- `ul-unas`: Rust streaming output correlates **0.957 at lag 0** with the
  repo's shipped enhanced sample (the residual comes from the repo's
  offline `center=True` padding and the 16 k↔48 k resampling chain).
- Listening-quality judgments are **not** made on this machine; that is
  exactly what the CLI comparison mode and the menu-bar model selector are
  for.
