#!/usr/bin/env python3
"""Verify a noican comparison manifest and every referenced float WAV."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import BinaryIO


def read_chunk(file: BinaryIO) -> tuple[bytes, bytes] | None:
    header = file.read(8)
    if not header:
        return None
    if len(header) != 8:
        raise ValueError("truncated RIFF chunk header")
    chunk_id, size = struct.unpack("<4sI", header)
    payload = file.read(size)
    if len(payload) != size:
        raise ValueError(f"truncated {chunk_id!r} chunk")
    if size % 2:
        file.read(1)
    return chunk_id, payload


def verify_wav(path: Path, expected_samples: int) -> None:
    with path.open("rb") as file:
        header = file.read(12)
        if len(header) != 12:
            raise ValueError(f"{path}: truncated RIFF header")
        riff, _, wave = struct.unpack("<4sI4s", header)
        if riff != b"RIFF" or wave != b"WAVE":
            raise ValueError(f"{path}: not a RIFF/WAVE file")
        format_chunk: bytes | None = None
        data_chunk: bytes | None = None
        while chunk := read_chunk(file):
            chunk_id, payload = chunk
            if chunk_id == b"fmt ":
                format_chunk = payload
            elif chunk_id == b"data":
                data_chunk = payload
        if format_chunk is None or data_chunk is None:
            raise ValueError(f"{path}: missing fmt or data chunk")
        audio_format, channels, sample_rate, _, _, bits = struct.unpack(
            "<HHIIHH", format_chunk[:16]
        )
        if audio_format == 0xFFFE:
            if len(format_chunk) < 40:
                raise ValueError(f"{path}: truncated WAVE_FORMAT_EXTENSIBLE chunk")
            audio_format = struct.unpack_from("<H", format_chunk, 24)[0]
        expected = (3, 1, 48_000, 32)
        actual = (audio_format, channels, sample_rate, bits)
        if actual != expected:
            raise ValueError(f"{path}: float WAV contract {actual}, expected {expected}")
        if len(data_chunk) != expected_samples * 4:
            raise ValueError(
                f"{path}: {len(data_chunk) // 4} samples, expected {expected_samples}"
            )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while block := file.read(64 * 1024):
            digest.update(block)
    return digest.hexdigest()


def verify(manifest_path: Path, expected_models: set[str]) -> None:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    records = {record["model"]: record for record in manifest["models"]}
    if set(records) != expected_models:
        raise ValueError(
            f"manifest models {sorted(records)} != expected {sorted(expected_models)}"
        )
    root = manifest_path.parent
    for model, record in records.items():
        if record["status"] != "complete":
            raise ValueError(f"{model}: {record['error']}")
        path = root / record["output"]
        verify_wav(path, manifest["input_samples"])
        actual_hash = sha256(path)
        if actual_hash != record["output_sha256"]:
            raise ValueError(
                f"{model}: SHA-256 {actual_hash} != {record['output_sha256']}"
            )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--expect", required=True, help="comma-separated model slugs")
    args = parser.parse_args()
    verify(args.manifest, set(args.expect.split(",")))
    print(f"verified {args.manifest}")


if __name__ == "__main__":
    main()
