# 0016. Miscellaneous decisions

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## block-bench-robustness

Source: `crates/noican-models/examples/block_bench.rs`, `main`.

Clamped to one second so a `0` argument cannot reach the quantile
lookup with an empty vector.

## build-script-linker-path-contract

Source: `macos/Package.swift`, `package`.

scripts/build-macos-app.sh reads this "-L" line back to
decide where cargo must put libnoican_ffi.a, and fails
the build if the path does not match its TARGET. Keep
the flag and its path on one line in this shape.
