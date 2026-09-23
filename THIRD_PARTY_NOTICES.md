# Third-Party Notices

This project builds on the following third-party software and models.
Rust crate dependencies are tracked in `Cargo.lock` and license-checked in
CI by `cargo-deny` (permissive licenses only); the entries below cover
models, adapted code, and vendored assets.

## Model weights (downloaded at runtime, never redistributed here)

| Component | Source | License |
|---|---|---|
| FastEnhancer-B ONNX model (`fastenhancer_b`, 48 kHz) | https://github.com/aask1357/fastenhancer (release `onnx-48khz-v1`) | MIT |
| DPDFNet ONNX models (`dpdfnet2_48khz_hr`, `dpdfnet8_48khz_hr`) | https://github.com/ceva-ip/DPDFNet / https://huggingface.co/Ceva-IP/DPDFNet (redistributed by https://github.com/k2-fsa/sherpa-onnx) | Apache-2.0 |
| DeepFilterNet3 model bundle | https://github.com/Rikorose/DeepFilterNet (embedded via the `deep_filter` crate) | MIT OR Apache-2.0 |
| UL-UNAS streaming ONNX | https://github.com/Xiaobin-Rong/ul-unas | MIT |
| Hush ONNX bundle (`advanced_dfnet16k_*`) | https://github.com/pulp-vision/Hush / https://huggingface.co/weya-ai/hush | Apache-2.0 |

## Adapted code and vendored assets

- The DPDFNet streaming pipeline (`stages/dpdfnet.rs`) follows the
  reference implementations in sherpa-onnx (Apache-2.0,
  `online-speech-denoiser-stft-impl.h`) and ceva-ip/DPDFNet
  (Apache-2.0, `stream.py`).
- The UL-UNAS pipeline (`stages/ulunas.rs`) follows
  `ulunas_onnx/stream/ulunas_stream.py` (MIT).

## Key runtime dependencies (see Cargo.lock for the full tree)

- ONNX Runtime via the `ort` crate (MIT OR Apache-2.0; ONNX Runtime
  itself: MIT).
- `deep_filter` / DeepFilterNet (MIT OR Apache-2.0), including its tract
  inference stack (MIT OR Apache-2.0).
- `symphonia` (MPL-2.0) — CLI decoding of AIFF/AIFC, CAF, and M4A
  (AAC/ALAC) inputs. MPL-2.0 is file-level weak copyleft: unmodified use
  imposes no obligations on this application beyond source availability
  of the MPL-covered files.
- `crossbeam-queue` and `rtrb` (MIT OR Apache-2.0) — lock-free queues for
  the real-time path (stage switching, audio I/O rings).

## Virtual audio driver (separate GPL-3.0 program)

- BlackHole (https://github.com/ExistentialAudio/BlackHole, GPL-3.0,
  (c) Existential Audio Inc.) is vendored as the `external/blackhole`
  git submodule, pinned to the upstream release tag `v0.7.1`, and built
  unmodified into the separate `Noican.driver` bundle by
  `scripts/build-driver.sh` (build-time preprocessor customization only —
  the joycast.driver pattern; see docs/driver.md). The driver artifact is
  GPL-3.0; `LICENSE.driver` carries the notice, the source-availability
  statement, and the full license text, and is embedded in the bundle.
- The BlackHole name, logo, and branding are trademarks of Existential
  Audio Inc. and are not used by the Noican driver build.
- The build/install/uninstall script structure follows
  https://github.com/joymacstudio/joycast.driver (GPL-3.0); the scripts
  here are original to this repository.

## Adapted code (macOS transport and control plane)

- `crates/noican-coreaudio`, `crates/noican-ffi`, and `macos/` are ported
  from this repository's Phase 0 transport candidate branch
  (`cursor/phase-zero-engine-4f79`, same project/license), rewired onto
  the current engine crates.

## Policy notes

- GPL-licensed code (BlackHole, joycast.driver) is used **only** in the
  separate virtual-device driver and never linked into this application
  (docs/tech-research.md §11, docs/driver.md). Never add
  `external/blackhole` sources or objects to the app or crate targets.
- License status of all model weights must be re-verified at ship time if
  the app is ever sold or distributed (docs/tech-research.md §11).
  The weight licenses above do not settle commercial use on their own:
  the training-data terms count too. FastEnhancer's 48 kHz training data
  includes TUT Urban Acoustic Scenes 2018, whose license prohibits
  "selling or distributing the results or content achieved by use of
  the Work", so `fastenhancer-b` must not ship in a paid release as is.
