# Model Assets and Reproducible Comparisons

## Asset cache

`noican` keeps model files outside the repository and verifies every cached or downloaded file before inference. The default location is:

- macOS: `~/Library/Caches/noican/models`
- Linux: `${XDG_CACHE_HOME:-~/.cache}/noican/models`

Override it with `--model-dir`.

```bash
cargo run --release -- models fetch --all
```

Every public artifact URL is pinned to an immutable release or repository revision, and `crates/noican-models/src/assets.rs` records its SHA-256. A corrupt or changed cached file is deleted and fetched again; a mismatching download is never promoted from its `.partial` path.

DeepFilterNet3 uses the checksum-pinned stateful ONNX export from mellonella. Hush weights are embedded by the pinned `hush-vani` crate, so Hush has no separate cache file.

## CLI file comparison

Process one or more WAV files through the same stage implementations used by the live engine:

```bash
cargo run --release -- process \
  recordings/room-a.wav recordings/room-b.wav \
  --all-models \
  --enrollment-wav recordings/enrollment.wav \
  --output-dir output
```

Inputs may be integer PCM or 32-bit float WAV at any sample rate. They are downmixed to mono and converted to the 48 kHz pipeline rate. Each input produces:

```text
output/<input-stem>-<input-sha-prefix>/
├── comparison.json
├── fastenhancer-t/output.wav
├── fastenhancer-b/output.wav
├── ...
└── tse-conv-tasnet-48k/output.wav
```

`comparison.json` records the input digest, exact model slugs, delay policy, output paths, output digests, and any per-model failure. By default the declared live-path delay is removed so model outputs are sample-aligned for listening. Use `--preserve-delay` to retain live timing.

The checked fixture can be reproduced and processed with:

```bash
python3 scripts/generate-sample-wav.py
cargo run -- process fixtures/sample-noisy.wav \
  --model fastenhancer-t,fastenhancer-b,fastenhancer-s,\
dpdfnet2-48khz-hr,dpdfnet8-48khz-hr,deepfilternet3,ul-unas,hush
```

## TSE enrollment

`tse-conv-tasnet-48k` requires a separate 192-dimensional ECAPA-TDNN embedding. The CLI computes it from `--enrollment-wav` with the public SpeechBrain `spkrec-ecapa-voxceleb` conversion and pinned filterbank table. The enrollment recording must contain at least one second of the target speaker at a reasonable level, without other voices.

An already computed embedding can be supplied as a JSON array:

```bash
cargo run -- process input.wav \
  --model tse-conv-tasnet-48k \
  --embedding-json enrollment.json
```

### Current upstream access blocker

As verified on 2026-08-25, both unauthenticated resolve URLs under `penta2himajin/tse-conv-tasnet-48k` return HTTP 401. The pinned revision and SHA-256 values are recoverable from an independent verified downloader and are built into noican, but the bytes could not be independently fetched in this cloud run. Therefore noican does not use trust-on-first-use or redistribute those files.

To use assets obtained legitimately from the publisher:

1. Put `tse_prod_48k.onnx` and `tse_prod_48k.onnx.data` together under `<model-dir>/tse/`, or set an authorized Hugging Face token in `NOICAN_HF_TOKEN`.
2. Re-run `models fetch` or `process`. The cache verifies the pinned graph digest `71490a5a…` and sidecar digest `4b84f54b…` before loading.

The model card has been reported as CC BY 4.0, but it is currently inaccessible, and its assertion that all training data is CC BY 4.0 conflicts with DEMAND's CC BY-SA 3.0 metadata. Commercial redistribution requires legal review.

The ECAPA weights are Apache-2.0, but VoxCeleb's source-media copyright, publicity, privacy, and biometric-use implications are not resolved merely by the model license. Shipping enrollment requires a separate product/legal review.

## Model provenance

| Model | Source | Asset license recorded upstream |
|---|---|---|
| FastEnhancer T/B/S 48 kHz | `aask1357/fastenhancer`, release `onnx-48khz-v1` | MIT |
| DPDFNet2/8 48 kHz HR | `Ceva-IP/DPDFNet` revision `dd6818d…` | Apache-2.0 |
| DeepFilterNet3 | `penta2himajin/deepfilternet3-onnx` revision `daf50ae…`; upstream `Rikorose/DeepFilterNet` | Apache-2.0 export; upstream MIT OR Apache-2.0 |
| UL-UNAS | `Xiaobin-Rong/ul-unas` revision `00f7c70…` | MIT |
| Hush | `hush-vani` 0.1.1 with embedded `weya-ai/hush` weights | Apache-2.0 |
| TSE Conv-TasNet 48 kHz | `penta2himajin/tse-conv-tasnet-48k` revision `5d8934d…` | Reported CC BY 4.0; access and DEMAND provenance unresolved |
| ECAPA-TDNN conversion | `penta2himajin/ecapa-tdnn-onnx` revision `57bc773…`; upstream SpeechBrain model | Apache-2.0; VoxCeleb source-media rights need review |

Re-check every upstream license before distribution. This table is engineering provenance, not legal advice.
