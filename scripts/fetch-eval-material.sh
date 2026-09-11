#!/usr/bin/env bash
# Downloads a small set of 48 kHz interfering-speaker recordings for
# `noican eval` (docs/hush-48k-eval.md).
#
# Source: VCTK Corpus (version 0.92), University of Edinburgh, CSTR —
# Yamagishi, Veaux, MacDonald, "CSTR VCTK Corpus: English Multi-speaker
# Corpus for CSTR Voice Cloning Toolkit", 2019, https://doi.org/10.7488/ds/2645.
# License: Creative Commons Attribution 4.0 International (CC BY 4.0).
# The files are fetched through the Hugging Face datasets-server row API of
# the `sanchit-gandhi/vctk` mirror (48 kHz FLAC, `wav48_silence_trimmed`;
# the mirror interleaves the mic1 and mic2 takes of each sentence and only
# mic1 is kept), so nothing but curl and python3 is needed and the 11 GB
# corpus archive is never downloaded. The material is for local evaluation only
# and must not be committed to this repository.
#
# Usage: bash scripts/fetch-eval-material.sh [dest-dir] [utterances-per-speaker]
#   dest-dir                 default ~/Desktop/noican-eval/interferer
#   utterances-per-speaker   default 24 (≈ 80 s per speaker; at most 50)
set -euo pipefail

DEST="${1:-$HOME/Desktop/noican-eval/interferer}"
PER_SPEAKER="${2:-24}"
DATASET="sanchit-gandhi/vctk"
API="https://datasets-server.huggingface.co/rows?dataset=${DATASET}&config=default&split=train"

# Row offsets where these speakers start in the mirror (checked 2026-09-11):
# p226 — male, English (Surrey); p228 — female, English (Southern England).
# One male and one female voice so the metrics do not hinge on one timbre.
SPEAKERS=("p226:800" "p228:2000")

mkdir -p "$DEST"
for entry in "${SPEAKERS[@]}"; do
  speaker="${entry%%:*}"
  offset="${entry##*:}"
  echo "$speaker: fetching $PER_SPEAKER utterances from row $offset"
  # Two rows per sentence (mic1 + mic2); the API serves at most 100 rows.
  rows=$((PER_SPEAKER * 2)); rows=$((rows > 100 ? 100 : rows))
  curl -fsSL "${API}&offset=${offset}&length=${rows}" \
    | python3 -c '
import json, sys, urllib.request, pathlib
dest = pathlib.Path(sys.argv[1]); speaker = sys.argv[2]
for row in json.load(sys.stdin)["rows"]:
    r = row["row"]
    if r["speaker_id"] != speaker:
        continue
    name = pathlib.Path(r["file"]).name  # e.g. p226_177_mic1.flac
    if not name.endswith("_mic1.flac"):
        continue
    target = dest / name
    if target.exists():
        continue
    urllib.request.urlretrieve(r["audio"][0]["src"], target)
    print("  ", name, "-", r["text"])
' "$DEST" "$speaker"
  # The row offsets are pinned to the mirror's current ordering; if it
  # drifts, the speaker filter matches nothing and the loop above is a
  # silent no-op — turn that into an error the owner can act on.
  count=$(find "$DEST" -maxdepth 1 -name "${speaker}_*_mic1.flac" | wc -l | tr -d ' ')
  if [ "$count" -eq 0 ]; then
    echo "error: no ${speaker} files in $DEST — the mirror's row ordering has changed;" >&2
    echo "       update the row offsets in SPEAKERS (see the comment above it)" >&2
    exit 1
  fi
  echo "$speaker: $count files present"
done

echo
echo "Interferer material in $DEST ($(find "$DEST" -maxdepth 1 -name '*.flac' | wc -l | tr -d ' ') files)."
echo "Attribution: VCTK Corpus 0.92, CSTR, University of Edinburgh — CC BY 4.0."
echo "Pass them to noican eval as: --interferer $DEST/*.flac"
