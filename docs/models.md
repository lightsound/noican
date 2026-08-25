# Model Weights: Sources, Fetching, and Verification

Model weights are **never committed to this repository**. They are fetched
from the official distribution points below into the `models/` directory
(git-ignored) and verified against pinned SHA-256 digests.

```sh
# List models and their fetch status
cargo run -p noican-cli --release -- models

# Download everything that is freely fetchable
cargo run -p noican-cli --release -- fetch

# Download specific models
cargo run -p noican-cli --release -- fetch fastenhancer-t dpdfnet2
```

## Registry

| id | model | family | rate | backend | weights source | license |
|---|---|---|---|---|---|---|
| `fastenhancer-t/b/s/m/l` | FastEnhancer (ICASSP 2026) | denoise | 48 k | ONNX Runtime | [GitHub release `onnx-48khz-v1`](https://github.com/aask1357/fastenhancer/releases/tag/onnx-48khz-v1) | MIT |
| `dpdfnet2` | DPDFNet2 48 kHz HR | denoise | 48 k | ONNX Runtime | [sherpa-onnx release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speech-enhancement-models) | Apache-2.0 |
| `dpdfnet8` | DPDFNet8 48 kHz HR | denoise | 48 k | ONNX Runtime | [HF Ceva-IP/DPDFNet](https://huggingface.co/Ceva-IP/DPDFNet) (not on the sherpa release yet) | Apache-2.0 |
| `dfn3` | DeepFilterNet3 | denoise (baseline) | 48 k | tract (embedded in the `deep_filter` crate) | — (no download) | MIT OR Apache-2.0 |
| `ul-unas` | UL-UNAS (TASLP 2026) | denoise (low-latency) | 16 k | ONNX Runtime | [commit-pinned repo file](https://github.com/Xiaobin-Rong/ul-unas/tree/main/ulunas_onnx/onnx_models) | MIT |
| `hush` | Hush (Weya AI) | speaker suppression | 16 k | tract (`deep_filter` crate, Hush tarball) | [HF weya-ai/hush](https://huggingface.co/weya-ai/hush) | Apache-2.0 |
| `tse-48k` | tse-conv-tasnet-48k | speaker extraction (enrollment) | 48 k | ONNX Runtime | [HF penta2himajin/tse-conv-tasnet-48k](https://huggingface.co/penta2himajin/tse-conv-tasnet-48k) — **currently private, see below** | unknown |
| `ecapa-tdnn` | ECAPA-TDNN embedding (SpeechBrain export) | support (enrollment) | 16 k | ONNX Runtime | [HF penta2himajin/ecapa-tdnn-onnx](https://huggingface.co/penta2himajin/ecapa-tdnn-onnx) | Apache-2.0 |

Sample-rate/frame-size differences are absorbed by the engine
(`noican-core::FramedStage`): 16 kHz models are driven through a
fixed-ratio polyphase resampler and all models present the same 48 kHz
streaming interface.

## tse-48k availability (unverified with trained weights)

As of 2026-08-25 the Hugging Face repo `penta2himajin/tse-conv-tasnet-48k`
returns **HTTP 401** (the repo became private; earlier research confirmed
these exact file names/URLs). Consequences:

- `noican fetch` skips `tse-48k` by default and prints the reason.
- If you have access, set `HF_TOKEN` (or `NOICAN_HF_TOKEN`) and run
  `noican fetch tse-48k`.
- Alternatively place `tse_prod_48k.onnx` and `tse_prod_48k.onnx.data`
  manually under `models/tse-48k/`.
- The stage implementation was verified against a **structurally identical
  random-weight export** (generated with mellonella's
  `scripts/export_tse_onnx.py`, PyTorch↔ONNX Runtime parity max|Δ| =
  3.4e-8): the 89-state streaming protocol, enrollment-embedding input, and
  the CLI enrollment path all work. **Extraction quality with the trained
  weights is unverified** until the weights are obtainable.

TSE enrollment uses the author's own ECAPA-TDNN export plus a
SpeechBrain-compatible fbank reimplementation (golden-tested against the
Python reference) so the embedding distribution matches what the TSE model
was trained on.

## Batch comparison (CLI file mode)

```sh
# All fetched models, one output directory per input file
cargo run -p noican-cli --release -- process my_recording.wav --out-dir out

# Specific models; comma-separated or repeated
cargo run -p noican-cli --release -- process my_recording.wav --models fastenhancer-s,dpdfnet2

# Speaker extraction with enrollment (3–10 s of your clean voice)
cargo run -p noican-cli --release -- process my_recording.wav --models tse-48k --enroll my_voice.wav
```

Outputs land in `out/<input-stem>/<model-id>.wav` (mono 48 kHz, 16-bit)
next to `reference.wav` (the input converted to mono 48 kHz), so files are
directly comparable in any editor/player. Inputs of any rate/channel count
are accepted; processing runs in realtime-sized blocks through the same
stage code the live engine uses, and each stage's buffering latency is
compensated so outputs are time-aligned with the reference.

## Verification status (2026-08-25, Linux x86_64)

- All denoise models produce finite, time-aligned, plausibly denoised
  output from a real noisy-speech sample.
- `dpdfnet2`: Rust output correlates **0.998 at lag 0** with an
  independent Python reference implementation of the same pipeline.
- `ul-unas`: Rust streaming output correlates **0.957 at lag 0** with the
  repo's shipped enhanced sample (the residual comes from the repo's
  offline `center=True` padding and the 16 k↔48 k resampling chain).
- `fbank` (enrollment features): max |Δ| ≈ 1.2e-3 dB against the Python
  SpeechBrain reference dump.
- `tse-48k`: mechanism verified with random weights only (see above).
- Listening-quality judgments are **not** made on this machine; that is
  exactly what the CLI comparison mode and the menu-bar model selector are
  for.
