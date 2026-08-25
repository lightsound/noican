#!/usr/bin/env python3
"""Generate the deterministic noisy WAV used by cloud integration checks."""

from __future__ import annotations

import argparse
import math
import random
import struct
import wave
from pathlib import Path

SAMPLE_RATE = 48_000
DURATION_SECONDS = 2
PEAK_I16 = 32_767


def sample(index: int, rng: random.Random) -> float:
    """Return one speech-like tone mixed with deterministic room noise."""
    time = index / SAMPLE_RATE
    syllable = 0.5 + 0.5 * math.sin(2.0 * math.pi * 3.2 * time)
    voice = syllable * (
        0.30 * math.sin(2.0 * math.pi * 180.0 * time)
        + 0.16 * math.sin(2.0 * math.pi * 360.0 * time + 0.2)
        + 0.08 * math.sin(2.0 * math.pi * 540.0 * time + 0.7)
    )
    hum = 0.08 * math.sin(2.0 * math.pi * 60.0 * time)
    broadband = 0.06 * (2.0 * rng.random() - 1.0)
    click_phase = index % 9_600
    click = 0.15 * math.exp(-click_phase / 180.0) if click_phase < 1_200 else 0.0
    return max(-1.0, min(1.0, voice + hum + broadband + click))


def generate(path: Path) -> None:
    """Write a mono, 16-bit, 48 kHz WAV."""
    path.parent.mkdir(parents=True, exist_ok=True)
    rng = random.Random(0x4E4F4943)
    with wave.open(str(path), "wb") as writer:
        writer.setnchannels(1)
        writer.setsampwidth(2)
        writer.setframerate(SAMPLE_RATE)
        frames = bytearray()
        for index in range(SAMPLE_RATE * DURATION_SECONDS):
            value = round(sample(index, rng) * PEAK_I16)
            frames.extend(struct.pack("<h", value))
        writer.writeframes(frames)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("fixtures/sample-noisy.wav"),
    )
    args = parser.parse_args()
    generate(args.output)


if __name__ == "__main__":
    main()
