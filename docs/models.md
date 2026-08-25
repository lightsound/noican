# Models

Weights are downloaded on demand and verified against a SHA-256 digest recorded
in `crates/noican-models/src/catalog.rs`. They are far too large to commit, and
a truncated download produces a graph that loads, runs, and outputs
plausible-sounding rubbish — which is much harder to diagnose than a checksum
failure.

## Getting the weights

```bash
# Everything in the catalog (~158 MB).
cargo run -p noican-cli -- fetch

# Or just what you need.
cargo run -p noican-cli -- fetch fastenhancer-t dpdfnet2-48k-hr

# What is in the catalog and what is on disk.
cargo run -p noican-cli -- list --detail

# Re-verify digests of what is already downloaded.
cargo run -p noican-cli -- verify
```

Files land in `models/<model-id>/`, or under `$NOICAN_MODEL_DIR` if that is set.
Downloads stream to a `.part` file and are renamed only once complete, so an
interrupted transfer can never be mistaken for a usable model.

## The catalog

| ID | Native rate | Block | Measured delay | Notes |
|---|---|---|---|---|
| `fastenhancer-t` | 48 kHz | 512 | 512 smp (10.7 ms) | 28K parameters; the primary candidate for the live path |
| `fastenhancer-s` | 48 kHz | 512 | 512 smp (10.7 ms) | Recommended starting point for quality comparisons |
| `fastenhancer-b` | 48 kHz | 512 | 512 smp (10.7 ms) | 207K parameters |
| `fastenhancer-m` | 48 kHz | 320 | 704 smp (14.7 ms) | Heavier than the published latency figures cover |
| `fastenhancer-l` | 48 kHz | 200 | 824 smp (17.2 ms) | Quality ceiling, not for the live path |
| `dpdfnet2-48k-hr` | 48 kHz | 480 | 2400 smp (50.0 ms) | The only 48 kHz DPDFNet published; claims some dereverberation |
| `dpdfnet2-16k` | 16 kHz | 160 | 800 smp (50.0 ms) | 16 kHz sibling of the above |
| `dpdfnet4-16k` | 16 kHz | 160 | 800 smp (50.0 ms) | Middle of the 16 kHz range |
| `dpdfnet8-16k` | 16 kHz | 160 | 800 smp (50.0 ms) | Heaviest DPDFNet published |
| `gtcrn` | 16 kHz | 256 | 256 smp (16.0 ms) | 48K parameters; the simplest graph in the catalog |
| `ul-unas` | 16 kHz | 256 | 256 smp (16.0 ms) | GTCRN's successor; the low-latency-mode candidate |
| `deepfilternet3` | 48 kHz | 384000 | 1440 smp (30.0 ms) | The reference baseline. A **block stage**: ~8 s of latency, offline use only |
| `hush` | 16 kHz | 128000 | 160 smp (10.0 ms) | Separates overlapping speakers. Also a block stage. Attenuates ~14 dB even on clean speech, which is the model's own behaviour |
| `speaker-gate` | 16 kHz | 1600 | none | Attenuates when the dominant speaker is not the enrolled one. Needs `noican enroll` first |

Delays were measured rather than assumed — no published export declares one.
Reproduce with:

```bash
cargo run -p noican-cli -- latency --probe path/to/real-speech.wav
```

The probe has to be a **real recording**. Speech-enhancement models correctly
treat synthetic tones as non-speech and suppress them, leaving nothing to
correlate; the built-in synthetic probe is a convenience for a quick look, not a
substitute.

`Measured delay` covers the model plus, for the spectral models, our own
analysis/synthesis round trip. It does not include the block accumulation and
resampling that `StageRunner` adds on top; `noican process` reports the
end-to-end figure per model.

### The speaker gate needs enrolling first

`speaker-gate` is the only model that needs something the catalog cannot
download: a recording of the voice it should keep.

```sh
noican fetch speaker-gate
noican enroll me-talking.wav                 # several recordings are better
noican process meeting.wav -o out -m speaker-gate
```

`enroll` averages an embedding over overlapping windows and reports how well
those windows agree with each other. A low figure almost always means the
recording had more than one voice in it.

The gate needs about 1.5 seconds of speech before it can recognise anyone, which
is a property of the embedding model rather than a tuning choice
(`docs/tech-research.md` §6.4). So it suppresses a sustained other voice, not a
single interjected word, and it starts open — audio is never gated before the
model has decided anything. It complements `hush`, which separates overlapping
speakers within a frame but cannot be told who you are.

### Two of them are block stages

The catalog marks these two as not live-capable, and the menu bar picker says
"offline only, seconds of latency" next to them. Without that, choosing one
looks indistinguishable from the app hanging.

`deepfilternet3` and `hush` ship as *sequence* graphs whose recurrent state is
not exposed, so they cannot be driven frame by frame. They run a block at a time
instead, with a warm-up context long enough that the result matches
`DeepFilterNet`'s own runtime exactly — which costs about eight seconds of
latency and 50 % more compute than the audio strictly needs. That is fine for
the comparison they exist for, and it rules them out of the live path until the
graphs are re-exported with explicit state (`docs/tech-research.md` §5.5).

Because of that block size they also need at least ~8 seconds of input before
they produce anything.

## Comparing models

```bash
# Every model over one file, aligned and ready for A/B listening.
cargo run --release -p noican-cli -- process recording.wav

# One directory per input:
#   out/recording/00-reference-unprocessed.wav   the input, same rate and length
#   out/recording/fastenhancer-t.wav             one file per model
#   out/recording/manifest.md                    delay, speed, peak, RMS per model
```

Outputs are aligned to the reference so that switching between files compares
the models rather than a time offset. Alignment is measured from the signal by
default (`--align measured`); `--align reported` uses the declared delay and
`--align none` leaves the output as produced.

### Record the comparison material at 48 kHz

A 48 kHz model fed 16 kHz audio upsampled to 48 kHz sees a signal with a brick
wall at 7.2 kHz and nothing above it, and treats the result as unnatural:
FastEnhancer attenuates the speech band by 4–16 dB on such material and is
within 0.3 dB of transparent on a real 48 kHz recording. Comparing the 48 kHz
and 16 kHz models on upsampled material will rank them for a reason that has
nothing to do with how they will sound in a meeting.

## Adding a model

1. Add a `ModelDescriptor` to `CATALOG` with the URL, the SHA-256 digest, the
   licence, and the source. Pin repository-tree URLs to a commit; a branch URL
   is not reproducible.
2. If its ONNX signature matches an existing `Architecture`, that is all — the
   engine, the CLI, and the UI pick it up. Otherwise add one `Stage`
   implementation for the new signature.
3. Measure its delay with `noican latency` and record it in
   `crates/noican-models/src/latency.rs`. A missing entry is reported as zero
   delay, which understates it.
4. Add its licence and attribution to `THIRD_PARTY_NOTICES.md`.
5. Confirm it is transparent on clean speech before trusting it. A denoiser that
   attenuates clean speech is misdriven, and the size of the attenuation says
   how badly — this is what caught a 16-bit WAV polarity bug and an
   undrivable third-party export during Phase 0 (`docs/tech-research.md` §5.5).
