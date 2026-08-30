# Technology Research: JoyCast-Style Noise-Cancelling Virtual Microphone for macOS

- **Date**: 2026-08-24 (five research rounds conducted on this date; round 5 was a zero-based sweep for alternative architectures and produced no stack changes — the stack below is final)
- **Status**: Research complete; implementation not started
- **Scope**: macOS only, personal use, fully on-device, Apple Developer Program membership available (Developer ID signing is possible)

This document consolidates the review of two earlier AI-generated design documents ("design1" and "design2") and three rounds of follow-up research. It records every candidate that was evaluated — including rejected ones and the reasons for rejection — so that later decisions can be revisited with full context and rejected options can serve as fallbacks.

---

## 1. Problem Statement

Replace the built-in (or any physical) microphone with a "clean microphone" selectable from any app. The pipeline:

```
Physical mic (48 kHz)
      ↓
[ Capture ]                 AUHAL (direct HAL I/O)
      ↓
[ Noise suppression ]       streaming neural model
      ↓
[ Background-speaker
  suppression ]             the key differentiator vs. JoyCast
      ↓
[ (optional) AEC ]          only if speaker playback is used
      ↓
[ Virtual mic device ]      HAL AudioServerPlugIn
      ↓
Zoom / Meet / Teams / OBS select it as input
```

Performance targets (matching JoyCast's published numbers): ~20 ms latency, ~75–100 MB memory, 48 kHz native, "removes up to 97% of background noise".

Key facts about JoyCast itself (confirmed during research):

- Its virtual driver is an open-source BlackHole fork: [joymacstudio/joycast.driver](https://github.com/joymacstudio/joycast.driver) (GPL-3.0).
- The product is a solo-developer app ($8/mo, also on Setapp); the AI model it uses is not published but is widely assumed to be DeepFilterNet-family.

---

## 2. Review Verdict on the Two Source Design Documents

Two AI-generated design documents were used as the starting point (Japanese, kept outside this repository).

| Topic | design1 | design2 | Verdict after research |
|---|---|---|---|
| Virtual device | Reuse BlackHole; libASPL if self-building | Custom HAL plugin as the goal; NNA as interim | Fork BlackHole via the joycast.driver pattern (see §3) — better than both |
| Inference route | Rust `df` crate (tract backend) | DeepFilterNet3 → CoreML / Neural Engine | Both superseded: ONNX Runtime / sherpa-onnx with newer models (see §5) |
| CoreML feasibility | "ONNX export lacks DSP; CoreML route too costly" | Assumed straightforward | design1 was right about the cost, but OSS apps (NoNoise-Mac, MetalVoice) have since implemented the Swift STFT/ERB pipeline, weakening the objection |
| Clock drift | Aggregate Device with drift compensation (mandatory) | Not mentioned | design1 correct; this is mandatory (§4) |
| Real-time constraints | No malloc/locks/ARC on the audio thread | Not mentioned | design1 correct (§9) |
| Capture API | AUHAL direct | AVAudioEngine first, Core Audio later | design1 correct — AVAudioEngine cannot target arbitrary HAL devices at all (§4) |
| Background speakers | DIY speaker gate (embedding + fade) | Not addressed | Pretrained models now exist (Hush, tse-conv-tasnet-48k); DIY gate becomes the fallback (§6) |
| AEC | Headphones-only; VoiceProcessingIO "needs testing" | Barely addressed | Headphones for v0 confirmed; a real solution exists now: process tap + WebRTC AEC3 (§7) |
| "Sidon" model (design2) | — | Proposed as high-quality option | Real but offline-only (dataset cleansing); not usable live (§8) |
| "NNA Virtual Audio" (design2) | — | Proposed as BlackHole upgrade | Real (free, renamable, closed-source); superseded by the signed BlackHole fork |
| UI | None initially (CLI + launchd) | SwiftUI menu bar from Phase 2 | Menu bar UI deferred but planned (design2's Phase 2 feature list adopted) |

Overall: design1 was the technically reliable backbone; design2 contributed the product/UI phasing. Every layer was subsequently upgraded by the research below.

---

## 3. Virtual Device Layer

### 3.1 Platform constraints (unchanged through macOS 26)

- **AudioServerPlugIn (HAL plugin) is the only sanctioned path** for virtual audio devices. Apple explicitly states AudioDriverKit entitlements **will not be granted** for virtual (non-hardware) drivers ([WWDC21 session 10190](https://developer.apple.com/videos/play/wwdc2021/10190/), [AudioDriverKit docs](https://developer.apple.com/documentation/audiodriverkit/creating-an-audio-device-driver)). The ASPL interface is not deprecated.
- CMIO extensions (`CMIOExtensionDevice`) cover virtual *cameras* only; not applicable to a system-wide virtual microphone.
- Core Audio process taps capture audio but cannot expose a selectable input device to other apps.
- macOS 26 (Tahoe) added no new virtual-device API.
- **Signing requirement**: on macOS 15+, `coreaudiod` only loads `.driver` bundles signed with a Developer ID (ad-hoc signatures work only on SIP-relaxed dev machines). We have a Developer ID, so this is not a blocker.
- HAL plugins live in `/Library/Audio/Plug-Ins/HAL/`, load inside `coreaudiod` (a separate process from our app), require `sudo` to install and a `coreaudiod` restart to activate. App ↔ driver data exchange requires POSIX shared memory + a ring buffer — unless the driver is a plain loopback, which is what we choose.

### 3.2 Candidates

| Candidate | Type | License | Notes |
|---|---|---|---|
| **joycast.driver pattern (BlackHole fork)** ✅ | BlackHole as git submodule + build-time renaming | GPL-3.0 | [joymacstudio/joycast.driver](https://github.com/joymacstudio/joycast.driver). Ships JoyCast itself. Customization is isolated to GCC preprocessor definitions, so upstream BlackHole updates merge trivially. Includes build, signing, PKG, install/uninstall scripts. **Chosen approach**: same pattern, our own name/ID, signed with our Developer ID. GPL is irrelevant for personal, non-distributed use |
| BlackHole (stock) | Prebuilt signed driver | GPL-3.0 | [ExistentialAudio/BlackHole](https://github.com/ExistentialAudio/BlackHole). Zero effort, but device shows up as "BlackHole 2ch" and the single-file C codebase is hard to modify directly |
| NNA Virtual Audio | Prebuilt signed driver | Free, closed-source | [neutralandnaturalaudio.com](https://neutralandnaturalaudio.com/virtual-audio.html). 1–256 configurable channels, renamable without reinstall, per-channel volume. Good no-build fallback; closed source is the drawback |
| LitLink | Prebuilt signed driver + companion app | Free (freemium) | [litpads.app/litlink](https://litpads.app/litlink). One-click multi-output / mic passthrough. More consumer-oriented than we need |
| AudioRouterNow | Open-source HAL driver + helper | GPL-3.0 | [mauriciomorkun/AudioRouterNow](https://github.com/mauriciomorkun/AudioRouterNow). Output fan-out focus; useful as another shared-memory ring-buffer reference |
| libASPL | C++17 framework for custom ASPL | MIT | [gavv/libASPL](https://github.com/gavv/libASPL). Production-proven via [roc-vad](https://github.com/roc-streaming/roc-vad) (which also demonstrates gRPC-controlled device lifecycle). The conservative choice if we ever need a fully custom driver (custom controls, in-driver features) |
| tympan-aspl | Rust framework for custom ASPL | (young, v0.1) | [penta2himajin/tympan-aspl](https://github.com/penta2himajin/tympan-aspl). Safe Rust over the ASPL C ABI, lock-free SPSC ring, realtime-safe primitives, loopback/gain/lowpass examples, author's guide. Keeps a Rust-core project single-language. Immature but promising |
| KraspHAL / NoNoise Mic drivers | Drivers bundled in OSS apps | MIT | From [Krasp](https://github.com/pilshchikov/krasp) / [NoNoise-Mac](https://github.com/ivalsaraj/NoNoise-Mac). Useful as reference code; would need re-signing anyway |
| AudioDriverKit (dext) | ❌ Rejected | — | Entitlements not granted for virtual devices |
| AUv3 plugin | ❌ Rejected | — | Loadable in DAWs/OBS but not selectable as a mic in Zoom/Meet |
| DSP inside the driver | ❌ Rejected | — | `coreaudiod` cannot open other HAL devices from within a plugin (re-entrancy); heavy inference inside `coreaudiod` is unacceptable anyway |

### 3.3 Decision

**Fork BlackHole using the joycast.driver pattern, rename, sign with our Developer ID.** The driver remains a dumb, named loopback; all DSP stays in the app process. Full custom drivers (libASPL / tympan-aspl) are deferred until a concrete need appears (e.g., in-driver controls or status properties).

---

## 4. Capture Layer, Clock Drift, and Routing

### 4.1 Capture and output

- Use **AUHAL directly** (`kAudioUnitSubType_HALOutput`) or `AudioDeviceCreateIOProcIDWithBlock` on the target device. Buffer size 128–256 frames at 48 kHz.
- **Do not use AVAudioEngine for device-specific I/O.** Its `inputNode` cannot be retargeted to a non-default HAL device: setting `kAudioOutputUnitProperty_CurrentDevice` returns `noErr` but is silently ignored (AUHAL logs `-10877` to Console only). Confirmed by a 2026 field report ([DGR Labs](https://dgrlabs.co/blog/2026-04-25-capturing-system-audio-on-macos-in-2026.html)). This invalidates design2's "start with AVAudioEngine" plan.
- Microphone TCC permission (`NSMicrophoneUsageDescription`) required.
- **Raw mic-array access is impossible on macOS** (checked as a beamforming angle): Apple's driver performs locked-down adaptive beamforming on the built-in 3-mic array and exposes a single processed stream; no public API returns the raw channels. [Triforce](https://github.com/chadmed/triforce) implements an MVDR beamformer for this array but only on Linux/Asahi. DIY beamforming is therefore a dead end on macOS; the pipeline is single-channel by constraint, not by choice.

### 4.2 Clock drift (mandatory countermeasure)

The physical mic and the virtual device belong to different clock domains (e.g., 48000.0 vs. 47999.8 Hz). Unhandled, the ring buffer slowly over/underflows, producing periodic clicks (typically every few minutes).

1. **Aggregate Device (chosen)**: combine physical mic + virtual device into one aggregate via `AudioHardwareCreateAggregateDevice`, designate a clock master, enable drift compensation (`kAudioSubDeviceDriftCompensationKey`; for taps, `kAudioSubTapDriftCompensationKey`). Use `kAudioAggregateDeviceIsPrivateKey` to hide it from device lists.
2. Fallback: DIY asynchronous sample-rate conversion, adjusting the ratio from ring-buffer occupancy.

Open question: whether aggregate drift compensation alone stays glitch-free over multi-hour meetings — must be verified with a long-run test.

---

## 5. Noise Suppression Models

### 5.1 Inference runtime — key finding

design1 concluded the Rust `df` crate was the only practical route because DeepFilterNet's public ONNX exports contain only the neural nets (STFT, ERB feature extraction, complex-filter application, and ISTFT must be reimplemented). This premise is outdated:

- **[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)** (k2-fsa, actively maintained) now ships **streaming speech enhancement including the STFT pipeline**, with C/C++/Swift/Rust/Python/etc. bindings ([PR #3324](https://github.com/k2-fsa/sherpa-onnx/pull/3324)). Supported: GTCRN and DPDFNet. The same library also provides Silero VAD and speaker-embedding models — one dependency can cover denoise + VAD + speaker verification.
- **FastEnhancer** ships its own ONNX streaming inference for plain ONNX Runtime.
- The original `df` crate still works but upstream DeepFilterNet development has stalled.

### 5.2 Candidates (real-time, streaming-capable)

| Model | Params | Sample rate | Availability | Assessment |
|---|---|---|---|---|
| **FastEnhancer** (ICASSP 2026) ✅ | 28K–207K (T/B/S) | 16 k & **48 k native** | [aask1357/fastenhancer](https://github.com/aask1357/fastenhancer): checkpoints + streaming ONNX | Explicitly optimized for lowest real-world latency on one CPU thread; claims SOTA quality among lightweight streaming models. WASM port ([fastenhancer-web](https://github.com/ryyr-ry/fastenhancer-web)) measures 0.45–3.9 ms per 10.67 ms frame at 48 kHz — native ARM64 will be faster. **Primary candidate** |
| **DPDFNet** (ceva-ip) ✅ | 2.3–3.6 M | 8/16/**48 k HR** | Official ONNX via [sherpa-onnx](https://k2-fsa.github.io/sherpa/onnx/speech-enhancement/dpdfnet.html) | DeepFilterNet2 + Dual-Path RNN; effectively the maintained DFN successor with graded quality/compute variants (`dpdfnet2_48khz_hr`, `dpdfnet8_48khz_hr`). **Primary candidate** (easiest integration) |
| DeepFilterNet3 | 2.1 M | 48 k native | Rust `df` crate (tract); ONNX (neural nets only) | The historical baseline; 10 ms hop; runs ~1/3 real time on Apple Silicon CPU. Upstream stalled. Keep as reference in listening tests |
| **UL-UNAS** (IEEE TASLP 2026) ✅ | ~171K | 16 k | [Xiaobin-Rong/ul-unas](https://github.com/Xiaobin-Rong/ul-unas): checkpoints + streaming ONNX (2026-02) | GTCRN author's successor; PESQ 3.09 vs. GTCRN 2.87 on VCTK-DEMAND. **Low-latency-mode candidate** |
| GTCRN | 48K | 16 k | sherpa-onnx | RTF 0.07 on a 2022 desktop CPU; clearly beats RNNoise. Superseded by UL-UNAS but has the smoother integration path today |
| RNNoise | 60K | 48 k | BSD, tiny | Two generations behind (GTCRN → UL-UNAS); keep only as a trivial fallback |
| CoreML DFN3 (Swift) | 2.1 M | 48 k | [NoNoise-Mac](https://github.com/ivalsaraj/NoNoise-Mac) / [MetalVoice](https://github.com/Ghostkwebb/MetalVoice) (MIT) implement the full Swift STFT/ERB pipeline; [speech-swift](https://github.com/soniqo/speech-swift) ships DFN3 as CoreML (INT8/FP16) + vDSP feature pipeline | Viable Swift-native route; per-project code quality unverified. Caveat: speech-swift's CoreML export does not yet thread GRU state as explicit I/O (60 s single-shot cap, chunked long-form) — file-oriented today, not mic-streaming-ready. Note: DFN3 is small enough that the Neural Engine offers little benefit over CPU |

### 5.3 Rejected for the live path

| Model | Reason |
|---|---|
| ZipEnhancer (Alibaba, ICASSP 2025) | SOTA PESQ but 62.4 GFLOPS — far too heavy for a low-latency CPU path |
| MossFormer2_SE_48K (ClearerVoice-Studio) | 48 kHz and high quality, but maintainers confirm it breaks with decode windows < 1 s ([issue #101](https://github.com/modelscope/ClearerVoice-Studio/issues/101)) — offline only. **Useful as the offline quality ceiling in listening tests** |
| "DeepFilterNet4" ([sealad886 fork](https://github.com/sealad886/DeepFilterNet4)) | Community fork with native MLX implementation; unofficial, single contributor — monitor only |
| FRCRN / MossFormerGAN (16 k) | Offline-oriented ClearerVoice models; not streaming |

---

## 6. Background-Speaker Suppression (the differentiator)

DeepFilterNet-class models learn speech-vs-noise separation: vacuum cleaners and keyboards disappear, but **a family member talking nearby or a TV voice passes straight through** ("speech looks like speech"). Solving this for one known user is the core advantage of a personal build. Three approaches, in order of preference:

### 6.1 Pretrained models (new since the original designs)

| Model | Approach | Sample rate | Status | Assessment |
|---|---|---|---|---|
| **Hush** (Weya AI, Apache-2.0) | DFN3 architecture retrained with 60% of samples containing competing speakers (12–24 dB SIR below primary); auxiliary separation head at training time only | 16 k | [pulp-vision/Hush](https://github.com/pulp-vision/Hush): PyTorch + prebuilt `libweya_nc.dylib` (Apple Silicon) with a 10 ms-frame C API; ONNX bundle on HF; "louder background speech" retrain announced | **First candidate to test.** No enrollment: suppresses the *background* (quieter) speaker, so it can fail if the interferer is louder than the user. Same inference cost as DFN3. 16 kHz output is the main quality concession. Already used in production-ish OSS (Krasp) |
| **tse-conv-tasnet-48k** | Causal streaming Conv-TasNet TSE conditioned on a frozen 192-dim ECAPA-TDNN enrollment embedding (FiLM) | **48 k native** | [HF: penta2himajin/tse-conv-tasnet-48k](https://huggingface.co/penta2himajin/tse-conv-tasnet-48k): per-chunk (10 ms, 480-sample) streaming ONNX with explicit state tensors; Rust wrapper (`TseSession`) in the mellonella project | **Second candidate.** True voiceprint enrollment at the native rate — exactly what design1 wanted but believed unavailable. Caveats: trained only on VCTK + DEMAND, v3 after two broken releases, solo-dev PoC parent project — quality unproven, must be validated by listening test |

### 6.2 DIY hard gate (fallback; validated design)

Speaker-embedding gate: run VAD + speaker verification (ECAPA-TDNN embedding, cosine similarity vs. an enrolled mean embedding) per frame window; fade out frames that do not match. All components are available pretrained (sherpa-onnx provides both VAD and speaker embeddings; also `voxudio`; on the Swift side, [speech-swift](https://github.com/soniqo/speech-swift) bundles Silero VAD, WeSpeaker ResNet34 embeddings (256-dim), and pyannote segmentation/diarization).

Two independent public PoCs implement exactly this design, confirming its soundness (and its non-novelty):

- [mellonella](https://github.com/penta2himajin/mellonella) — `input → DFN3 (NS) → [VAD + SV + F0] decision → gate → output`; design spec + Python PoC, Rust port planned. Explicitly accepts that simultaneous overlapping speech cannot be separated by a gate (FP-tolerant policy).
- [voce](https://github.com/espetro/voce) — enrollment (mean L2-normalized embedding over 1 s windows) + cosine gate + 3-frame sliding vote + BlackHole passthrough; documents the thread model (fail-open when the inference thread lags).

Known tuning risk (from design1): gate fade time constant — too short clips the user's speech onsets; too long leaks the interferer.

### 6.3 Research-only options (not adoptable now)

| Work | Why not |
|---|---|
| TargetVoice (Interspeech 2025) | 22 MB low-latency TSE, impressive numbers, but no public model release found |
| SpeakerBeam-SS (Interspeech 2024) / [OpenSpeakerBeam-SS](https://github.com/helloooideeeeea/openspeakerbeam-ss) | Real-time TSE with S4D; open reimplementation exists but early-stage, license TBD |
| D-LGTSE / SEF-PNet / USEF-PNet / DSEF-PNet | Embedding-free PSE research line (ICASSP 2025+); 8–16 k research code, no production-ready streaming release |
| [Look Once to Hear](https://github.com/vb000/LookOnceToHear) (CHI 2024 honorable mention) | Binaural (two-ear mic) enrollment and processing — hardware model mismatch with a single Mac mic |
| [RAVEN](https://github.com/Bose/RAVEN) (Bose, Interspeech/WASPAA 2025) | First open-source **real-time audio-visual** speech enhancement: webcam watches the on-screen speaker's lips and isolates them; runs on Apple Silicon CPU. Rejected for the mic path because the visual encoder needs a 5-video-frame buffer → **~120 ms algorithmic latency** (vs. our 20–30 ms budget), plus webcam dependency. The strongest "revolutionary" watch item if a low-latency visual encoder appears |
| Voice-conversion architecture (e.g. [LLVC](https://github.com/KoeAI/LLVC), <20 ms any-to-one VC on CPU) | Considered as a radical alternative: resynthesize everything as the user's voice, making noise structurally impossible. Rejected: any-to-one VC **converts background speakers' speech into the user's voice too** — the exact opposite of speaker isolation — and adds naturalness risk |
| TSE via Positive/Negative enrollments (NeurIPS 2025) | TFGridNet-based, research code |
| ClearerVoice-Studio TSE | Audio-only variant is 8 kHz; AV variants need camera input |
| VoiceFilter-Lite / Personalized PercepNet | Historical options from design1 — no usable public pretrained models (unchanged) |

### 6.4 Decision

Listening-test **Hush (16 k, no enrollment)** vs. **tse-conv-tasnet-48k (48 k, enrollment)** on recordings that include real family/background speech. If neither satisfies, build the DIY gate (§6.2) on sherpa-onnx primitives. Combining is also possible (e.g., FastEnhancer 48 k for NS + gate for speakers).

---

## 7. Acoustic Echo Cancellation (AEC)

Needed only when the user plays meeting audio through speakers; a virtual mic in the path can degrade the meeting app's own AEC (reference-signal mismatch).

### 7.1 Apple VoiceProcessingIO (VPIO) — limitations confirmed

- `setVoiceProcessingEnabled(true)` enables AEC + NS + AGC as a bundle.
- AGC alone can be disabled (`kAUVoiceIOProperty_VoiceProcessingEnableAGC = 0`); **NS and AEC cannot be separated** — a dedicated final check confirmed the only public properties are the AGC toggle, output mute, and the all-or-nothing `kAUVoiceIOProperty_BypassVoiceProcessing`. No NS-specific property exists through macOS 26. This question is closed.
- Field report ([Forasoft](https://www.forasoft.com/ship-log/spatial-audio-vpio)): VPIO builds an aggregate around the output device and demands a stereo reference; **fails to initialize (err -10875) on Spatial Audio Macs** and is flaky on Bluetooth/aggregate outputs. Their fix was replacing VPIO's NS with RNNoise and abandoning VPIO — reinforcing our decision to avoid it.
- Double-processing concern (VPIO NS + our neural NS) remains, so even for AEC-only use it is unattractive.

### 7.2 New solution: Core Audio process tap + WebRTC AEC3

- **Reference signal**: macOS 14.2+ [Core Audio process taps](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps) (`AudioHardwareCreateProcessTap` + `CATapDescription`) capture system/process output digitally — no driver needed, insertable into an aggregate device, with `kAudioSubTapDriftCompensationKey` for drift. Requires the system-audio-capture TCC prompt (`NSAudioCaptureUsageDescription`).
- **AEC**: [`aec3` crate](https://crates.io/crates/aec3) — Rust port of WebRTC AEC3 with graph/pipeline API, built-in delay estimation, optional NS/AGC2/HPF nodes, 10 ms frames. Author labels it "still validating". Alternative: C++ `webrtc-audio-processing`.
- Lifecycle reference: [CoreAudioTapKit](https://github.com/CJStanfield/CoreAudioTapKit) (Swift; owns tap + aggregate + ring buffer + AUHAL output). Known pitfalls it handles: tap creation succeeds with no permission but delivers zero callbacks; must exclude own process from the tap; must wait for the aggregate to report alive.
- **macOS 26 (Tahoe) regression**: taps can intermittently deliver all-zero buffers or a non-firing IOProc (Apple Dev Forums thread 825780; reproduced with Teams). Mitigation pattern from production apps ([dimmy commit](https://github.com/KonradDallaOrg/dimmy/commit/a81a902a6de7a2dfb8624aaff9edba0a576471ce)): non-zero-signal watchdog → full teardown + rebuild of tap + aggregate. Also: on macOS 26 the Screen Recording TCC grant implicitly covers system-audio capture, and a "System Audio Recording Only" TCC subsection exists.

### 7.3 Neural AEC options (alternatives to WebRTC AEC3)

Small neural AEC models have matured and offer what Apple never exposed — echo cancellation as a standalone module (all 16 kHz):

| Model | Size | Form | Notes |
|---|---|---|---|
| [LocalVQE](https://github.com/richiejp/LocalVQE) v1.4-AEC | 203K params (~3 MB); also a 2.7K linear-filter variant | GGML (C++), causal streaming, 16 ms hop | DeepVQE derivative. **Echo-only variant keeps voice, noise, and room** — composes cleanly with our own NS stage. Joint AEC+NS+dereverb variants (1.3–4.8 M) also available ([HF](https://huggingface.co/LocalAI-io/LocalVQE)) |
| [JointAEC-NS](https://github.com/miuda-ai/joint_aec_ns) | 55K params (~440 KB) | Streaming ONNX, 10 ms frames, MIT | Joint AEC+NS in one graph; single-thread CPU RTF 0.015. Would replace both the AEC and low-latency NS stages at 16 kHz |
| EchoFree (arXiv:2508.06271) | 278K params | Paper (linear filter + Bark-scale neural post-filter) | Comparable to DeepVQE-S; no production release found |

These still need the process-tap reference signal (§7.2); they replace only the AEC3 DSP stage, potentially with better quality on nonlinear echo paths.

### 7.4 Decision

v0 operates headphones-only (no AEC). When speaker use becomes a requirement, implement the process-tap reference plus either WebRTC AEC3 (`aec3` crate; 48 kHz-friendly, battle-tested algorithm) or LocalVQE v1.4-AEC (neural, echo-only, 16 kHz) — decided by an echo-path test at that time. VPIO is avoided entirely.

---

## 8. "Studio-Grade" Restoration / Enhancement (future work)

| Option | Status | Assessment |
|---|---|---|
| Light DSP chain (HPF, EQ, compressor, limiter via Accelerate/vDSP or fundsp) | Trivial | The practical way to add "studio polish" after NS; adopt when needed |
| [Sidon](https://github.com/sarulab-speech/sidon) (UTokyo sarulab, MIT) | Real, but offline | w2v-BERT 2.0 feature cleanser + vocoder resynthesis, 48 kHz out, 100+ languages; built for TTS dataset cleansing with ~10 s chunking. Ports exist (CoreML/ONNX via Soniqo). **Offline post-processing option only** (e.g., cleaning recordings), never live |
| [Stream.FM / MelFlow](https://github.com/sp-uhh/streamfm) (TASLP 2026 / ICASSP 2026, AGPL-3.0) | Real, streaming, 48 kHz variant; SE + dereverb + BWE + codec post-filter; 24–48 ms total latency | First genuinely streaming generative restoration — but requires a consumer **CUDA GPU** (CUDA graphs) to hit real time; no Apple Silicon path today; AGPL. **Watch item**: generative restoration is coming to real time; revisit in 1–2 years |
| [LLaSE-G1](https://github.com/ASLP-lab/LLaSE-G1) (ACL 2025, Apache-2.0) | Real; checkpoints on [HF](https://huggingface.co/ASLP-lab/LLaSE-G1) | LLaMA-based unified generative model covering NS, TSE, AEC, packet-loss concealment, and separation in one checkpoint (WavLM features → X-Codec2 tokens). **Offline only** (two-stage LM inference, no streaming path, known instability); watch item — a future streaming successor would collapse several pipeline stages into one model |
| [mlx-audio](https://github.com/Blaizzy/mlx-audio) / [speech-swift](https://github.com/soniqo/speech-swift) MLX ports | Real | DeepFilterNet-mlx, MossFormer2_SE_48K_MLX, SAM-Audio (text-guided source separation) on Apple GPU via MLX. Useful for offline batch work on-device; the streaming mic path is better served by CPU ONNX (GPU contention, scheduling jitter) |
| resemble-enhance, AnyEnhance, Miipher(-2) | Offline / closed | Not applicable |

Dereverberation note: DPDFNet variants claim some dereverb capability; MossFormer2_SE_48K and Sidon handle it offline. If live dereverb matters, compare DPDFNet HR against FastEnhancer in the listening test.

---

## 9. Real-Time Engineering Constraints

(From design1; confirmed unchanged and worth restating as hard rules.)

- Never run inference in the audio I/O callback. Inference runs on a dedicated thread; hand-off via lock-free ring buffers (SPSC).
- Forbidden on the audio thread: heap allocation, locks/condvars, Swift ARC retain/release, file I/O, logging, blocking syscalls.
- Join the processing thread to the device's Audio Workgroup (`os_workgroup`) for correct scheduling.
- Rust (or C) for everything the real-time constraint touches; Swift only outside it (UI, control plane) or with audited allocation-free callbacks.

Latency budget (target 20–30 ms end-to-end):

| Component | Budget |
|---|---|
| Input buffer (128 frames @ 48 kHz) | ~2.7 ms |
| Model hop (10 ms frame models) | 10 ms |
| Model algorithmic delay | ~10–30 ms (model-dependent; FastEnhancer/Hush ≈ 20 ms class) |
| Ring buffers + virtual device output | ~5 ms |

---

## 10. Existing OSS Applications (references / parts donors / escape hatches)

| Project | Stack | License | Value to us |
|---|---|---|---|
| [Krasp](https://github.com/pilshchikov/krasp) | Swift 6.1 + Rust; **Hush**/DeepFilterNet; own `KraspHAL.driver`; menu bar; meters; strength control; GitHub Actions PKG builds | MIT | The closest existing implementation to our target architecture (validates Hush-in-production). Early-stage (0 stars), unsigned builds. Reference for the app-side plumbing and the Swift+Rust split |
| [NoNoise-Mac](https://github.com/ivalsaraj/NoNoise-Mac) | Swift/SwiftUI + CoreML DFN3 (`computeUnits = .all`); faithful Swift STFT/ERB pipeline; bundled "NoNoise Mic" driver; menu bar + CLI | MIT | Proves the CoreML/Swift route; parts donor for vDSP feature pipeline and driver packaging |
| [MetalVoice](https://github.com/Ghostkwebb/MetalVoice) | NoNoise's upstream; CoreML DFN3 + BlackHole output | MIT | Same as above (earlier iteration) |
| [joycast.driver](https://github.com/joymacstudio/joycast.driver) | Shell/AppleScript build system around BlackHole submodule | GPL-3.0 | **Adopted** as the virtual-device template (§3) |
| [roc-vad](https://github.com/roc-streaming/roc-vad) | C++ libASPL driver + gRPC control + CLI | MPL-2.0 | Reference for a fully custom driver with runtime device management, if ever needed |
| [mellonella](https://github.com/penta2himajin/mellonella) / [voce](https://github.com/espetro/voce) | Rust/Python PoCs of speaker gating | — | Design references for the DIY gate (§6.2) |
| [speech-swift](https://github.com/soniqo/speech-swift) | Swift toolkit: DFN3 + Sidon (CoreML), Silero/pyannote VAD, WeSpeaker embeddings, diarization, ASR/TTS | MIT | The richest Swift-native parts source if the app side grows (offline denoise, enrollment tooling, VAD/embeddings for the DIY gate). Mic-streaming DFN3 not yet supported (§5.2) |
| Buy option | [JoyCast](https://joycast.ai/) ($8/mo), Krisp, macOS Voice Isolation | — | JoyCast remains the shortest path to "quiet meetings" without the speaker-suppression differentiator |

Apple built-in note: macOS **Voice Isolation** mic mode cannot be enabled programmatically (`preferredMicrophoneMode` is read-only; users toggle it per-app in Control Center, and only apps adopting AUVoiceIO expose it). It is not a substitute for a virtual-mic product, but it is a zero-effort baseline for personal calls in supported apps. The macOS 26 **SpeechAnalyzer** framework was also checked: it is speech-to-text only (with a `SpeechDetector` VAD module) and offers nothing for the enhancement path.

Research dead ends worth recording: the Microsoft **DNS Challenge** series (the main engine of personalized-NS research) ended with ICASSP 2023; the winning TEA-PSE 3.0 system was never released as a usable model, so no new challenge-driven model supply should be expected from that direction.

### Competitive landscape note (2026)

As of 2026, **Microsoft Teams ships personalized voice isolation** (30-second voice-profile enrollment, personalized on-device model) and **Zoom ships personalized audio isolation** (locally stored voiceprint, optional scripted enrollment). Google Meet still uses a global, non-personalized model. Implication for positioning: "suppress other people's voices" is becoming a built-in feature *inside* Teams and Zoom, so this product's differentiation is being the **universal, system-wide layer** — one clean microphone that works identically in Google Meet, Discord, OBS, FaceTime, Slack huddles, recording apps, and anything else, with the user's own choice of model quality (48 kHz path vs. the platforms' internal processing) and full local privacy. NoNoise-Mac also demonstrates a feature worth borrowing later: cleaning **incoming** audio (what you hear) via a Core Audio process tap — the same tap machinery already planned for AEC (§7.2).

---

## 11. Final Recommended Stack

| Layer | Primary | Fallback / notes |
|---|---|---|
| Virtual device | BlackHole fork via joycast.driver pattern, Developer ID signed | Stock BlackHole or NNA (no build); libASPL / tympan-aspl (full custom) |
| Capture & output | AUHAL direct (`AudioDeviceCreateIOProcIDWithBlock`), 128–256 frame buffers | — (AVAudioEngine is not an option for device-targeted I/O) |
| Drift | Private Aggregate Device with drift compensation | DIY adaptive resampler |
| NS model (quality) | FastEnhancer 48 k **or** DPDFNet 48 k HR — decided by listening test | DeepFilterNet3 (`df` crate); CoreML DFN3 route |
| NS model (low-latency mode) | UL-UNAS | GTCRN (easier integration via sherpa-onnx) |
| Background speakers | Hush 16 k vs. tse-conv-tasnet-48k — decided by listening test | DIY VAD + ECAPA gate (mellonella/voce design) |
| AEC | None in v0 (headphones) | Process tap + `aec3` (WebRTC AEC3) with macOS 26 watchdog |
| Inference runtime | ONNX Runtime (FastEnhancer, TSE); sherpa-onnx (DPDFNet/GTCRN, VAD, speaker embeddings) | tract via `df` crate |
| Core language | Rust (audio engine, inference, gating) | — |
| UI | SwiftUI `MenuBarExtra` from the start (on/off, input device picker, status; strength/meters/mode switch and `SMAppService` login item added incrementally). Rust engine embedded as a static library behind a C ABI, or run as a separate daemon process with a small IPC control plane | CLI + config + launchd (if the UI ever blocks progress) |
| Enhancement extras | vDSP/fundsp EQ + compressor (later) | Sidon offline cleanup; Stream.FM (watch) |

### Licensing notes for commercial distribution

Personal (non-distributed) use carries no obligations. If the app is ever **sold or distributed**, the stack splits as follows (not legal advice; re-verify licenses at ship time):

**Permissive — safe for closed-source commercial use** (attribution/notice files required):
FastEnhancer (MIT), DPDFNet code + models (Apache-2.0), Hush (Apache-2.0), DeepFilterNet (MIT/Apache-2.0 dual), sherpa-onnx (Apache-2.0), ONNX Runtime (MIT), libASPL (MIT), Sidon (MIT), speech-swift (MIT), JointAEC-NS (MIT), Krasp / NoNoise-Mac / MetalVoice (MIT), webrtc-audio-processing (BSD-3).

**Copyleft but workable — obligations attach to the driver only**:
The BlackHole-fork driver (GPL-3.0) is a separate program loaded by `coreaudiod`, not linked into the app, so the GPL does not extend to the app itself. Distribution requires publishing the driver source under GPL-3.0 — exactly what JoyCast does with [joycast.driver](https://github.com/joymacstudio/joycast.driver), which is the precedent for this model. Note that the GPL explicitly permits **selling** ("You may charge any price or no price for each copy"); the only obligation is source access for the GPL-covered component. Since publishing source is acceptable for this project, no paid license is needed. Alternatives if source publication ever becomes undesirable: Existential Audio offers commercial BlackHole licenses (no public pricing; individual negotiation via devinroth@existential.audio). Separately from the GPL, the **BlackHole name, logo, and branding are Existential Audio trademarks** (all rights reserved) — the fork must ship under our own name, which the joycast.driver build-time renaming already handles.

**Not usable in a commercial product**:
NNA Virtual Audio (free *for personal use*; commercial use requires a vendor license with no public pricing — contact@neutralandnaturalaudio.com. Dropped from consideration for any sold version), Stream.FM (AGPL-3.0 — would force open-sourcing the entire app).

**Verify before shipping** (license not yet confirmed):
tse-conv-tasnet-48k model weights (HF card lacks an explicit license; trained on VCTK CC-BY-4.0 + DEMAND), LocalVQE weights, tympan-aspl, `aec3` crate (upstream WebRTC is BSD-3), UL-UNAS, GTCRN.

---

## 12. Roadmap

> Revised at the start of implementation: the former Phase -1 (offline listening
> test) is **merged into Phase 0**. Instead of a one-shot offline comparison that
> picks a single model up front, the engine is built around a **switchable model
> architecture** from day one, so models are compared both offline (CLI batch
> mode, strictly identical conditions) and live (switching from the UI during
> real meetings). A new model is added by implementing one trait.

### Cross-cutting: quality gates (from the very first commit)

Policy: **start with everything as an error, and demote only what is truly
impossible, with a written reason.**

- Rust: `[workspace.lints]` denies an exhaustive rustc lint set
  (`unsafe_code`, `missing_docs`, `unreachable_pub`, trivial casts, the
  `unused` group, ...), the rustdoc lints, and clippy with the `pedantic` /
  `nursery` / `cargo` groups promoted to error. Local opt-outs require
  `#[expect(..., reason = "...")]` (stale opt-outs fail the build;
  `clippy::allow_attributes` is denied), falling back to `#[allow]` with a
  reason only where `expect` cannot work (cfg-dependent code).
- `cargo-deny` (licenses, advisories, duplicate/unknown sources) and
  `cargo-machete` (unused dependencies) run in CI (GitHub Actions) together
  with `cargo fmt --check`, clippy, rustdoc, and tests.
- Swift side (in CI on a macOS runner): SwiftLint in strict mode and
  `swift build -Xswiftc -warnings-as-errors`; the same runner lints and
  tests the whole Rust workspace for `aarch64-apple-darwin` and builds the
  release app bundle.
- TS/JS: none in the repo today. PR #2 (fallow + ImportLint, branch
  `cursor/lint-tooling-3722`) was closed for that reason; revive that branch if
  TS/JS ever enters the repo.

### Phase 0 — Switchable engine + CLI comparison + minimal UI

> **Status (2026-08-25): code-complete.** Phase 0 was implemented as a
> best-of-3 (three parallel agents, tasks 0–4); candidate C's engine won and
> was merged to `main` (Rust workspace, stage abstraction, model registry
> with SHA-256-verified fetching, CLI batch comparison). Candidate B's macOS
> transport (`PR #4`, branch `cursor/phase-zero-engine-4f79`) **passed
> hardware acceptance on macOS 26 / Apple Silicon** — mic permission →
> Running, QuickTime recording through BlackHole 2ch, no crash or blowup on
> live model switching, clean teardown — while its *engine* failed
> acceptance (stereo input collapsed to the left channel in the CLI; the
> 16 kHz resampling path, especially Hush, was near-silent in real time;
> transient sounds such as keystrokes/claps cut off speech). Candidate A
> (`PR #3`) contributed the exhaustive lint configuration. The completed
> Phase 0 is therefore a hybrid: **C engine + B transport**, with B's
> coreaudio/FFI/Swift layers rewired onto the C engine crates so the live
> path uses exactly the resampling/framing code the CLI verifies, plus A's
> quality gates. The CLI additionally reads AIFF/AIFC/CAF/M4A (symphonia)
> besides WAV. Remaining for Phase 0 sign-off: the on-hardware acceptance
> run of this hybrid build (docs/macos-hardware-test.md) — audio output,
> TCC, aggregate-device behavior, and model switching cannot be verified
> in CI and are **not claimed as working** until recorded there. Note for
> all macOS 26 documentation: `sudo launchctl kickstart -k
> system/com.apple.audio.coreaudiod` is rejected by SIP; restart the audio
> daemon with `sudo killall coreaudiod` instead.

Everything the former Phase -1 needed, built as the product itself:

- **Rust workspace** with the quality gates above wired into CI from the first
  commit.
- **Common stage abstraction**: every model (denoiser or speaker-suppressor) is
  a `Stage` trait implementation. The trait absorbs per-model sample-rate and
  frame-size differences (internal resampling/buffering), so the engine always
  sees 48 kHz frames. Adding a future model = one new trait impl.
- **Model lineup** (all via ONNX Runtime unless noted; sources in §14):
  FastEnhancer 48 k (T/B/S first, M/L optional), DPDFNet 48 k HR
  (`dpdfnet2_48khz_hr`, `dpdfnet8_48khz_hr`), DeepFilterNet3 (baseline),
  UL-UNAS (16 k low-latency), Hush 16 k (no enrollment), tse-conv-tasnet-48k
  (enrollment via an external 192-dim ECAPA-TDNN embedding — the TSE
  distribution does not include the embedding model; use a public ECAPA ONNX
  from sherpa-onnx or SpeechBrain).
- **Model weights**: downloader (or documented manual steps) fetching from the
  official releases listed in §14; weights are never committed to the repo.
- **CLI file mode**: batch-process WAV files through any/all models with
  per-model output directories — the strict same-conditions comparison the old
  Phase -1 wanted, now reproducible at any time.
- **Real-time pipeline**: physical mic → currently selected model → BlackHole
  (stock, unmodified), private aggregate device for drift compensation, AUHAL
  direct I/O (§4.1). **Runtime model switching is lock-free** (atomic swap of
  the active stage; the audio thread never blocks) and masked by a short
  crossfade/mute so switching produces no clicks.
- **Minimal SwiftUI `MenuBarExtra`** from day one: on/off toggle, input device
  picker, **model selector** (the live listening-test control), running status.
  Rust engine embedded as a static library behind a C ABI (single app bundle)
  or a child/daemon process with a small IPC control plane — decided during
  implementation.
- Select BlackHole as input in Zoom. Already usable daily; model comparison
  happens during real use.

Exit criteria: converge on a preferred NS model and speaker-suppression
approach from real-world use (the selector stays — it is also the escape hatch
when a model misbehaves in a specific room); or conclude quality is
insufficient and fall back to buying JoyCast.

### Phase 1 — Own the device + differentiator

- Build, sign, and install the renamed BlackHole fork (joycast.driver pattern).
- Promote the best speaker-suppression stage (Hush / TSE / DIY gate per
  Phase 0 findings) to a default-on, tuned feature.

### Phase 2 — Menu bar app, full version

Extend the Phase 0 UI: strength control, quality/low-latency mode switch, level + reduction meters (lock-free shared state from the engine), start at login via `SMAppService`. (SwiftLint strict and Swift warnings-as-errors have been in CI since Phase 0.)

### Phase 3 — Optional

- AEC via process tap + AEC3 (if speaker use emerges).
- Incoming-audio cleaning ("clean what you hear") via the same process tap — NoNoise-Mac precedent.
- Light EQ/compressor polish; offline Sidon cleanup for recordings.
- Re-evaluate generative restoration (Stream.FM-class) for Apple Silicon, and audio-visual enhancement (RAVEN-class) if visual-encoder latency drops.

---

## 13. Open Questions

1. Listening-test outcomes (§12 Phase 0, CLI comparison + live switching) — the entire stack pivots on these.
2. Hush's behavior when the background speaker is *louder* than the user (trained at 12–24 dB SIR below primary).
3. tse-conv-tasnet-48k real-world quality given its small training set (VCTK + DEMAND).
4. Long-session (2 h+) stability of aggregate-device drift compensation.
5. DIY gate fade time constant (if the DIY route is needed): onset clipping vs. interferer leakage.
6. How meeting apps treat the virtual device's reported latency/safety offsets (BlackHole reports zero).
7. AEC engine choice, only relevant if AEC is built: `aec3` crate maturity vs. C++ `webrtc-audio-processing` vs. neural LocalVQE v1.4-AEC (16 kHz constraint).
8. Whether 16 kHz output (Hush path) is subjectively acceptable in meetings vs. the 48 kHz paths.

---

## 14. Reference Index

### Virtual device
- BlackHole: https://github.com/ExistentialAudio/BlackHole
- joycast.driver (BlackHole fork template): https://github.com/joymacstudio/joycast.driver
- NNA Virtual Audio: https://neutralandnaturalaudio.com/virtual-audio.html
- libASPL: https://github.com/gavv/libASPL / roc-vad: https://github.com/roc-streaming/roc-vad
- tympan-aspl: https://github.com/penta2himajin/tympan-aspl
- AudioRouterNow: https://github.com/mauriciomorkun/AudioRouterNow
- LitLink: https://litpads.app/litlink
- Apple, "Creating an Audio Server Driver Plug-in"; WWDC21 #10190 (AudioDriverKit virtual-device entitlement policy)

### Core Audio APIs
- Process taps: https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps
- CoreAudioTapKit: https://github.com/CJStanfield/CoreAudioTapKit
- AVAudioEngine device-targeting pitfall: https://dgrlabs.co/blog/2026-04-25-capturing-system-audio-on-macos-in-2026.html
- macOS 26 tap regression mitigation: https://github.com/KonradDallaOrg/dimmy/commit/a81a902a6de7a2dfb8624aaff9edba0a576471ce

### Noise suppression
- FastEnhancer: https://github.com/aask1357/fastenhancer (paper: arXiv:2509.21867) / WASM: https://github.com/ryyr-ry/fastenhancer-web
- sherpa-onnx speech enhancement: https://github.com/k2-fsa/sherpa-onnx (PR #3324) / DPDFNet docs: https://k2-fsa.github.io/sherpa/onnx/speech-enhancement/dpdfnet.html
- DeepFilterNet: https://github.com/Rikorose/DeepFilterNet
- GTCRN: https://github.com/Xiaobin-Rong/gtcrn / UL-UNAS: https://github.com/Xiaobin-Rong/ul-unas (arXiv:2503.00340)
- ZipEnhancer: arXiv:2501.05183 / ClearerVoice-Studio: https://github.com/modelscope/ClearerVoice-Studio

### Speaker suppression / TSE
- Hush: https://github.com/pulp-vision/Hush (model: https://huggingface.co/weya-ai/hush)
- tse-conv-tasnet-48k: https://huggingface.co/penta2himajin/tse-conv-tasnet-48k
- mellonella: https://github.com/penta2himajin/mellonella / voce: https://github.com/espetro/voce
- TargetVoice (Interspeech 2025), SpeakerBeam-SS (Interspeech 2024) / OpenSpeakerBeam-SS: https://github.com/helloooideeeeea/openspeakerbeam-ss
- D-LGTSE: https://github.com/isHuangZiling/D-LGTSE / SEF-PNet family: https://github.com/isHuangZiling/SEF-PNet
- Look Once to Hear: https://github.com/vb000/LookOnceToHear

### AEC
- aec3 (Rust WebRTC AEC3 port): https://crates.io/crates/aec3
- VPIO analysis: https://www.forasoft.com/ship-log/spatial-audio-vpio
- LocalVQE (neural AEC/VQE, GGML): https://github.com/richiejp/LocalVQE / https://huggingface.co/LocalAI-io/LocalVQE
- JointAEC-NS: https://github.com/miuda-ai/joint_aec_ns
- EchoFree: arXiv:2508.06271

### Restoration (offline / future)
- Sidon: https://github.com/sarulab-speech/sidon (JASA 10.1121/10.0040823)
- Stream.FM / MelFlow: https://github.com/sp-uhh/streamfm (arXiv:2512.19442)
- LLaSE-G1: https://github.com/ASLP-lab/LLaSE-G1 (arXiv:2503.00493)
- mlx-audio: https://github.com/Blaizzy/mlx-audio / speech-swift: https://github.com/soniqo/speech-swift

### Apps
- JoyCast: https://joycast.ai/
- Krasp: https://github.com/pilshchikov/krasp
- NoNoise-Mac: https://github.com/ivalsaraj/NoNoise-Mac / MetalVoice: https://github.com/Ghostkwebb/MetalVoice
