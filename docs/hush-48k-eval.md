# Evaluating "Hush at 48 kHz" candidates (`noican eval`)

The re-scoped Phase 1 item ([tech-research.md](tech-research.md) §6.4
decision record) is to deliver Hush's background-speaker suppression
without its 16 kHz bandwidth. Candidates for that are ranked first on
objective numbers from a controlled mixture, then decided by the owner's
ears on a blind listening set. Both come out of one CLI command:

```sh
cargo run -p noican-cli --release -- fetch hush-48k
cargo run -p noican-cli --release -- eval \
  --target  ~/Desktop/noican-eval/voice-builtin.m4a \
  --interferer ~/Desktop/noican-eval/interferer/*.flac \
  --models passthrough,hush,hush-48k,dfn3 \
  --out-dir ~/Desktop/noican-eval/out-builtin
```

`eval` loads weights from `--models-dir` (default `./models`, the same
directory `fetch` writes; [models.md](models.md)) and never downloads:
a listed model that is not fetched there stops the run with `cannot
create stage <id>`. `dfn3` needs no files.

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
   model) on each mixture in 480-sample blocks with the same stage code
   the live engine uses, compensates the stage's reported latency (as
   `process` does), and measures the wall-clock time of every block.
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

Record with QuickTime Player: File → New Audio Recording, open the
menu next to the record button, pick the **physical microphone** (not
"Noican Microphone") and quality **Maximum** (Apple Lossless at the
device rate, so nothing above 8 kHz is thrown away), record about
70 s, then File → Save as `~/Desktop/noican-eval/voice-builtin.m4a`.
The `.m4a` is accepted directly (ALAC and AAC are both decoded); no
conversion to WAV is needed. macOS ships no command-line recorder
(`afrecord` does not exist; `afplay`/`afconvert` are playback and
conversion only), so if a scripted recording is wanted, `ffmpeg -f
avfoundation` or `sox`/`rec` from Homebrew are the options.

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

The full sweep (`--target-level-dbfs`, SIR +12, `hush` and the
unleveled `hush-48k` behave alike in these columns):

| Voice RMS | SI-SDR you | level | resid all |
|---|---|---|---|
| −55 dBFS | −120 dB (output zeroed) | −49 dB | — |
| −45 | 4.7 dB | +1.1 dB | −18.2 dB |
| −40 | 9.1 dB | +1.3 dB | −17.7 dB |
| −38 | 14.6 dB | +0.9 dB | −2.1 dB |
| −37 | 14.4 dB | +0.4 dB | −1.4 dB |
| −34 | 10.2 dB | −1.8 dB | −0.5 dB |
| −30 | 4.5 dB | −4.6 dB | −1.0 dB |
| −22 | 1.7 dB | −7.8 dB | −2.3 dB |
| −15 | 6.1 dB | −8.8 dB | −2.3 dB |

Two readings. The own-voice columns have a narrow optimum at −38…−37:
hotter input is progressively attenuated and loses its 4–7 kHz band
(and, through the gate, the restored band), colder input has its quiet
syllables zeroed by the model's silence short-circuit. And `resid all`
flips from −18 dB to ≈ 0 dB between −40 and −38: on this material the
"suppression" of the lone interferer below −40 dBFS is Hush's level
gate, not speaker separation — another reason the stand-in `resid`
columns are diagnostic only.

### Input leveler in `hush-48k` (2026-09-24)

A 36-minute owner recording through `hush-48k` (voice median
−39 dBFS, loud sentences −30…−20) showed the output's share above
8 kHz falling 9 dB from the quietest to the loudest sentences — the
level dependence above, heard as brightness following the sentence
dynamics. `hush-48k` therefore trims input hotter than −35 dBFS down to
that level before the core and restores it on the output (attenuation
only, anchored to the loudest sustained talker, never below a floor on
the talker's sentence-scale level; design record in
`crates/noican-models/src/stages/leveler.rs`). The same sweep after the
change, `hush-48k` at SIR +12:

| Voice RMS | HF keep | level | SI-SDR you | SI-SDR both | resid HF |
|---|---|---|---|---|---|
| −45 dBFS | −4.0 dB | +1.1 dB | 4.7 dB | 12.4 dB | −9.5 dB |
| −37 | −4.0 dB | +0.8 dB | 14.3 dB | 8.4 dB | −4.3 dB |
| −30 | −6.0 dB | −0.2 dB | 14.3 dB | 6.6 dB | −4.3 dB |
| −22 | −7.1 dB | −1.6 dB | 13.6 dB | 6.8 dB | −4.5 dB |
| −15 | −7.1 dB | −1.8 dB | 12.9 dB | 6.8 dB | −4.7 dB |

Input at or below the target is untouched (the −45 row is the
unleveled stage), and the hot rows now sit within 1.5 dB of the −37 row
on SI-SDR and within 2.6 dB on level, against 4.5 / 1.8 / 6.1 dB and
−4.6 / −7.8 / −8.8 dB before. Per-second output/input level in the
you-only segment at −22 dBFS went from −17…+1 dB to −3…+1 dB (the
first two seconds of a session, while the trim settles, are the −3).
`SI-SDR both` is lower than without the floor (10.7 → 6.6 at −30): a
second talker heard during the owner's pauses is lifted onto the same
floor as the owner's own quiet sentences — the two are the same level
to a level cue — so it is the price of keeping those sentences.

The sweep scales one recording by one factor, so it cannot show the
trim following a talker whose sentences move. A stand-in with 4 s
sentences alternating −20 / −42 / −30 dBFS (`hush` is the unleveled
reference, same core; per-sentence output level and own-voice SI-SDR):

| sentence | hush | hush-48k |
|---|---|---|
| −21 dBFS | −4.0 dB, 12.5 dB | −2.4 dB, 12.6 dB |
| −48 | −9.7 dB, −7.1 dB | −2.4 dB, 9.1 dB |
| −36 | −3.4 dB, 11.5 dB | −2.1 dB, 15.7 dB |
| −17 | −8.2 dB, 6.5 dB | −0.8 dB, 16.7 dB |
| −41 | −13.3 dB, −5.4 dB | +0.1 dB, 13.5 dB |

Without the floor the −36 sentence, arriving after the anchor had
pinned to −21, reached the core at −50 dBFS and read −0.1 dB; the
floor returns it to 15.7 dB while leaving the loud sentences at the
unfloored figures. Sentences under −40 dBFS are not trimmed; they read
better than through the unleveled `hush` because the core's running
normalisation has seen a steadier level in the sentences before them.
The floor's peak-hold runs through pauses (its hold and fall are wall
time), so after a pause longer than a second the next sentence gets
its floor on its first frame; measured against a peak-hold that only
advanced on speech frames, the −48 / −41 sentences moved from −5.7 /
6.2 dB to 9.1 / 13.5 dB.

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

(Measured before the input leveler of 2026-09-24; at −37 dBFS the
leveled stage reads HF keep −4.0, level +0.8, SI-SDR you 14.3 — see
"Input leveler" above for the other levels.)

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
