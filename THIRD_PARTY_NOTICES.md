# Third-party notices

noican bundles no third-party source, but it downloads model weights at run
time and it was written with reference to several open-source projects. This
file records what came from where.

The virtual-audio driver is deliberately **not** listed here. It is a fork of
[BlackHole](https://github.com/ExistentialAudio/BlackHole) (GPL-3.0), which
`coreaudiod` loads as a separate program and which is never linked into this
workspace. Its obligations attach to the driver alone; see
`docs/tech-research.md` §11. No GPL-licensed code may enter `crates/` or
`apps/`.

## Model weights

Downloaded on demand by `noican fetch`; see `crates/noican-models/src/catalog.rs`
for the exact URLs and SHA-256 digests.

| Model | Licence | Copyright / source |
|---|---|---|
| FastEnhancer T / S / B / M / L | MIT | Hyungseob Lim et al. — <https://github.com/aask1357/fastenhancer> (ICASSP 2026, arXiv:2509.21867) |
| DPDFNet 2 / 4 / 8 and the 48 kHz HR variant | Apache-2.0 | Ceva Inc., redistributed via the sherpa-onnx `speech-enhancement-models` release — <https://github.com/k2-fsa/sherpa-onnx> |
| GTCRN (`gtcrn_simple`) | Apache-2.0 | Xiaobin Rong — <https://github.com/Xiaobin-Rong/gtcrn>, redistributed via sherpa-onnx |
| UL-UNAS (`ulunas_stream_simple`) | Apache-2.0 | Xiaobin Rong — <https://github.com/Xiaobin-Rong/ul-unas> (IEEE TASLP 2026, arXiv:2503.00340) |
| DeepFilterNet3 (`DeepFilterNet3_onnx.tar.gz`) | MIT OR Apache-2.0 | Hendrik Schröter et al. — <https://github.com/Rikorose/DeepFilterNet> |
| Hush | Apache-2.0 | Weya AI — <https://huggingface.co/weya-ai/hush> |
| ECAPA-TDNN (`ecapa_tdnn.onnx`) | Apache-2.0 | ONNX export of SpeechBrain's `spkrec-ecapa-voxceleb` — <https://huggingface.co/penta2himajin/ecapa-tdnn-onnx> |

## Reference implementations consulted

No code was copied verbatim; these are the sources the algorithms were derived
from, and each is credited at the point of use in the code.

| Project | Licence | What was taken |
|---|---|---|
| [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) (`libDF`) | MIT OR Apache-2.0 | The whole `DeepFilterNet` front-end in `crates/noican-models/src/dfn`: the ERB filter-bank construction, the exponential feature normalisation and its seed values, the analysis window scaling, the ERB mask interpolation, and the order-5 complex deep filter. Its `tract` runtime was also built locally and used as the reference our output is verified against |
| [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) | Apache-2.0 | How a DPDFNet graph is driven: the metadata contract, the layout of its state tensor, and where the normalisation seeds go inside it |
| [kaldi-native-fbank](https://github.com/csukuangfj/kaldi-native-fbank) | Apache-2.0 | The weighted-overlap-add normalisation used by the streaming inverse transform, and the Vorbis window definition |
| [GTCRN](https://github.com/Xiaobin-Rong/gtcrn) / [UL-UNAS](https://github.com/Xiaobin-Rong/ul-unas) | Apache-2.0 | The per-frame cache-threading protocol and each model's analysis window |
| [FastEnhancer](https://github.com/aask1357/fastenhancer) | MIT | The waveform streaming protocol: a hop-sized input chunk, an `n_fft - hop` output delay, and which caches hold the overlap buffers |

## Rust dependencies

Licences of the full dependency graph are enforced by `cargo-deny` against the
allow-list in `deny.toml`, which admits permissive licences only. Run
`cargo deny check licenses` to reproduce.

ONNX Runtime itself reaches the build through the [`ort`](https://github.com/pykeio/ort)
crate (MIT OR Apache-2.0), which downloads a prebuilt
[ONNX Runtime](https://github.com/microsoft/onnxruntime) (MIT) binary.
