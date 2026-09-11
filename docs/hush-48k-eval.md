# Evaluating "Hush at 48 kHz" candidates (`noican eval`)

The re-scoped Phase 1 item ([tech-research.md](tech-research.md) §6.4
decision record) is to deliver Hush's background-speaker suppression
without its 16 kHz bandwidth. Candidates for that are ranked first on
objective numbers from a controlled mixture, then decided by the owner's
ears on a blind listening set. Both come out of one CLI command:

```sh
cargo run -p noican-cli --release -- eval \
  --target  ~/Desktop/noican-eval/voice-builtin.wav \
  --interferer ~/Desktop/noican-eval/interferer/*.flac \
  --models passthrough,hush,hush-48k,fastenhancer-b \
  --out-dir ~/Desktop/noican-eval/out-builtin
```

Nothing under `~/Desktop/noican-eval/` is ever committed: the recordings
are personal and the public corpus material is redistributed under its
own license (below).

## What the command does

1. Reads the clean own-voice recording(s) (`--target`, any format the
   CLI decodes, converted to mono 48 kHz) and the interfering-speaker
   recording(s) (`--interferer`, several files are concatenated), trims
   leading/trailing silence (10 ms frames under −50 dBFS), and
   optionally normalizes the voice to `--target-level-dbfs`.
2. Builds one mixture per SIR (`--sir`, default `12,6,0` dB), three
   segments of `--segment-seconds` each (default 20 s, clamped to the
   material):

   ```text
   |---- you only ----|---- you + other (at SIR) ----|---- other only ----|
   0                  L                              2L                   3L
   ```

   The interferer gain is set so that the RMS ratio *inside the middle
   segment* equals the SIR exactly.
3. Runs every model (`--models`; default: passthrough plus every fetched
   model that needs no enrollment) on each mixture in 480-sample blocks
   with the same stage code the live engine uses, compensates the
   stage's reported latency (as `process` does), and measures the
   wall-clock time of every block.
4. Prints the metric table, writes `metrics.csv`, the mixtures
   (`sir+12/input.wav`, …), every output (`sir+12/<model>.wav`), the
   clean voice (`target.wav`), and a blind listening set
   (`sir+12/blind/A.wav`, `B.wav`, … with the answers in
   `blind-key.txt` at the top of the output directory).

## Metrics

All values in dB; definitions and their unit tests live in
`crates/noican-cli/src/eval.rs`. Every region skips its first 250 ms so a
model's reaction to a new condition is not scored.

| Column | Region | Meaning | Better |
|---|---|---|---|
| HF keep | you only | Energy of the output at or above 8 kHz relative to the clean voice. A 16 kHz model reads about −80 dB; a transparent 48 kHz path reads 0 dB | closer to 0 |
| level | you only | Output RMS relative to the clean voice RMS: loudness parity of your own voice. This is the number `HUSH_MAKEUP_GAIN_DB` compensates; a band-limited path reads slightly negative even when nothing else changes | closer to 0 |
| SI-SDR you | you only | Scale-invariant SDR of the output against the clean voice. Band limits, artefacts and pumping all lower it; a pure gain change does not (that is what `level` is for) | higher |
| SI-SDR both | you + other | The same while the interferer talks. The untouched mixture reads about the SIR; a suppressor should read above it | higher |
| resid all | other only | Output RMS relative to the mixture RMS: how much of the interferer is left when only the other person talks | more negative |
| resid HF | other only | The same, energy at or above 8 kHz only: the leak a band-split design shows if its upper band is not gated by Hush | more negative |
| latency | — | `Stage::latency_samples` in ms (the value the dry/wet mixer and the switch fade rely on) | — |
| p50 / p99 | — | Processing time of a 10 ms block, on the machine running the command (a `--release` build; the live budget is 5 ms of computation per block) | — |

Reading the table: `passthrough` is the anchor (0 dB, 0 dB, 100 dB, SIR,
0 dB, 0 dB). `hush` shows what is being preserved (its `resid` columns)
and what is being fixed (`HF keep` near −80 dB). A candidate is good
when its `HF keep` is near 0 dB **and** its `resid` columns are no worse
than Hush's — the high-band residual in particular must not rise.

The same first segment is used for every SIR, so `HF keep`, `level` and
`SI-SDR you` repeat across the SIR rows of one model.

## Material

### Interfering speaker: VCTK (CC BY 4.0)

`scripts/fetch-eval-material.sh [dest] [utterances-per-speaker]`
downloads 48 kHz FLAC utterances of two VCTK speakers (p226, male;
p228, female) — about 80 s each with the defaults — into
`~/Desktop/noican-eval/interferer/`:

- Source: **VCTK Corpus 0.92**, University of Edinburgh, CSTR (Yamagishi,
  Veaux, MacDonald, 2019, <https://doi.org/10.7488/ds/2645>), licensed
  **Creative Commons Attribution 4.0 International**. The script fetches
  single files through the Hugging Face datasets-server row API of the
  `sanchit-gandhi/vctk` mirror (`wav48_silence_trimmed`, mic1) so the
  11 GB archive is never downloaded; the files themselves are
  unmodified corpus audio at 48 kHz.
- Why VCTK: LibriSpeech (the other obvious CC BY 4.0 corpus) is 16 kHz,
  which would make the `resid HF` column trivially zero. VCTK is
  studio-recorded and close-miked, so it does not carry the room
  reverberation of a real person talking across the room; a recording
  of a real third person in the same room is the more faithful
  interferer when one is available.
- Note: Hush was trained with VCTK as *both* primary and background
  speech (speaker-disjoint splits), so a VCTK interferer is
  in-distribution for it.

### Own voice

Only the owner can provide this. Requirements: clean speech (no other
talkers, no music), 48 kHz, at least 60 s after trimming (2 × 20 s
segments; 40 s is the minimum for the default segment length, 2 s the
absolute minimum), recorded through the microphone the result is meant
for — the built-in microphone and the Shure MV7i are the two 48 kHz
inputs that can benefit from a 48 kHz path; Bluetooth HFP captures at
16 kHz and cannot exercise the high-band columns (the command warns
when a recording has almost no energy above 8 kHz).

Record with QuickTime Player (File → New Audio Recording, quality
**Maximum**, the physical microphone selected — not "Noican
Microphone"), or with `afrecord`:

```sh
# 70 s from the current default input, 48 kHz 16-bit mono WAV
afrecord -t 70 -f WAVE -d LEI16@48000 -c 1 ~/Desktop/noican-eval/voice-builtin.wav
```

(`afrecord` is part of macOS; select the input device in System
Settings → Sound before running it. If it rejects the format flags on
your macOS version, record with QuickTime instead.) `.m4a` from
QuickTime is accepted directly.

A **real third person** speaking in the same room for 60 s, recorded
the same way while the owner stays silent, is the best interferer. It
replaces (or is added to) the VCTK files via `--interferer`.

## Level dependence (measured 2026-09-11)

Hush's treatment of a voice depends on the absolute input level. With
VCTK p228 as a stand-in voice (Linux x86_64, `--release`, `hush` alone,
20 s of continuous speech), the output level relative to the input,
per second:

| Input RMS | Output − input, per second (dB) |
|---|---|
| −37 dBFS | 0, +1, +1, … −1, −2 (parity) |
| −25 dBFS (as recorded) | −2, −2, −1, −2, −2, −4, −7, −1, −12, −5, −2, −2, −12, −14, −5, −17, −13, −8, −12, −11 |
| −13 dBFS | −6, −5, −5, −7, −9, −6, −9, −8, −11, −9, −10, −13, −9, −8, −10, −5, −5, −5, −5, −5 |

Louder single-talker input is progressively treated as background;
at the built-in microphone's level (−36 dBFS RMS in the 2026-09-11
acceptance record) the voice passes at parity. Two consequences:

- Compare candidates at the level the microphone actually delivers.
  `--target-level-dbfs` exists to test the same voice at several
  levels; the recorded level is used when it is omitted, and the
  command prints the RMS of both recordings. Nothing is rescaled
  automatically: when a mixture or a model output exceeds full scale
  the command warns that the written WAVs (the listening set) are
  clipped while the metrics, computed on the unclamped floats, are not
  — lower the level or the SIR range before listening in that case.
- The VCTK stand-in numbers in this document are not a statement about
  the owner's voice; the owner's recording is what counts.

## Candidate `hush-48k` (stand-in numbers, 2026-09-11)

`hush-48k` (registry entry; design record in
`crates/noican-models/src/stages/hush_wideband.rs` and
[tech-research.md §6.4](tech-research.md)) adds the input's band above
8 kHz back to Hush's output, scaled by the gain Hush applied in
4–7 kHz. On VCTK stand-ins (Linux x86_64, `--release`; p228 as the
voice at −37 dBFS, p226 as the interferer):

| model | SIR | HF keep | level | SI-SDR you | SI-SDR both | resid all | resid HF | latency | p50/p99 |
|---|---|---|---|---|---|---|---|---|---|
| hush | +12 | −81.3 | +0.4 | 14.4 | 9.5 | −1.4 | −77.8 | 22.5 ms | 0.71/0.79 |
| hush-48k | +12 | −4.5 | +0.4 | 14.6 | 9.6 | −1.4 | −5.4 | 22.5 ms | 0.76/0.83 |
| hush | 0 | −81.3 | +0.4 | 14.4 | −3.7 | 0.1 | −78.6 | 22.5 ms | 0.71/0.78 |
| hush-48k | 0 | −4.5 | +0.4 | 14.6 | −3.7 | 0.1 | −6.6 | 22.5 ms | 0.76/0.85 |

Reading: the restored band follows Hush's own spectral tilt (Hush
leaves a passed voice at ≈ −6 dB in 6–7 kHz, so `HF keep` lands near
−5 dB rather than 0), latency and cost are unchanged, and the
interferer's high band is attenuated at least as much as its full band
(no high-band leak beyond what Hush itself lets through).

Caveat on the material: with two dry, close-miked studio voices Hush
barely separates them — `resid all` near 0 dB means it passed the lone
interferer as if it were the primary speaker. Hush's suppression was
established on the owner's live test (a real second person across the
room); the studio stand-in is diagnostic for the *high-band* columns
(does the added band follow Hush's decisions?), not for suppression
strength. The owner's own recordings, ideally with a real third
person, are the decisive test for both.

## Blind listening

`sir+XX/blind/` holds every model's output under a letter; the mapping
is in `blind-key.txt` (do not open it first). Listen to each letter at
the three points the segment line printed by the command gives, e.g.
with 20 s segments:

- **0–20 s (you only)** — does the voice sound like the 48 kHz
  microphone (air, sibilants) or like a telephone? Any warble, pumping,
  band seam?
- **20–40 s (you + other)** — is the other voice gone while you talk?
  Does your voice change when the other one starts?
- **40–60 s (other only)** — how much of the other voice remains, and
  is what remains hiss-like (high band leaking) or voice-like?

Write down the preferred letter per SIR, then read `blind-key.txt`.
