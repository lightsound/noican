#!/usr/bin/env python3
"""Comment ratchet: fail if the comment-line count of any source file grows.

Policy (2026-11-21): code comments in `crates/` and `macos/` are banned in
principle. Rationale belongs in `docs/adr/`; the code carries at most a
one-line ADR pointer where a lint (Rust `missing_docs`) forces a doc comment
to exist. The comments that remained after the 2026-11-21 delete-comment
pass are recorded in `scripts/comment-baseline.json`; this script fails CI
when any file's comment-line count exceeds its baseline entry, or when a
file with no baseline entry gains a comment. Counts may only ever shrink:
after deleting more comments, regenerate the baseline with `--update` and
commit the lowered numbers in the same change.

Exempt: `// swift-tools-version:` lines in Package.swift files. That line
wears comment syntax but is a SwiftPM directive, not a comment.

Usage:
    python3 scripts/comment-ratchet.py           # check against the baseline
    python3 scripts/comment-ratchet.py --update  # regenerate the baseline
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "comment-baseline.json")
SCAN_DIRS = ("crates", "macos")
EXTS = (".rs", ".swift")
SKIP_DIRS = {".git", "target", ".build"}
EXEMPT_PREFIX = "// swift-tools-version:"


def comment_lines(path):
    """Count lines that carry comment text (whole-line or trailing).

    String-aware: `//` inside a "..." literal, a char literal ('"'), or a
    Swift triple-quoted string is not a comment. Rust raw strings do not
    occur in this codebase; if one is ever introduced, extend this scanner
    in the same change.
    """
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    count = 0
    in_block = False
    in_ml_string = False
    for line in lines:
        if in_block:
            count += 1
            if "*/" in line:
                in_block = False
            continue
        if line.strip().startswith(EXEMPT_PREFIX):
            continue
        j = 0
        in_str = False
        while j < len(line):
            c = line[j]
            if in_ml_string:
                if line.startswith('"""', j):
                    in_ml_string = False
                    j += 3
                    continue
                j = len(line)
                break
            if in_str:
                if c == "\\":
                    j += 2
                    continue
                if c == '"':
                    in_str = False
                j += 1
                continue
            if line.startswith('"""', j):
                in_ml_string = True
                j += 3
                continue
            if c == '"':
                in_str = True
                j += 1
                continue
            if c == "'":
                # char literal ('x', '\\'', '"') vs lifetime ('a): skip the
                # literal form so a '"' inside it cannot mask a comment
                m = re.match(r"'(\\\\.|[^\\\\'])'", line[j:j + 4])
                if m:
                    j += len(m.group(0))
                    continue
                j += 1
                continue
            if c == "/" and j + 1 < len(line) and line[j + 1] == "/":
                count += 1
                break
            if c == "/" and j + 1 < len(line) and line[j + 1] == "*":
                count += 1
                if "*/" not in line[j + 2:]:
                    in_block = True
                break
            j += 1
    return count


def scan():
    counts = {}
    for top in SCAN_DIRS:
        base = os.path.join(ROOT, top)
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for fn in sorted(filenames):
                if fn.endswith(EXTS):
                    p = os.path.join(dirpath, fn)
                    n = comment_lines(p)
                    if n > 0:
                        counts[os.path.relpath(p, ROOT)] = n
    return counts


def main():
    current = scan()
    if "--update" in sys.argv:
        with open(BASELINE, "w", encoding="utf-8") as f:
            json.dump(current, f, indent=1, sort_keys=True)
            f.write("\n")
        print(f"baseline updated: {sum(current.values())} comment lines "
              f"in {len(current)} files")
        return 0
    with open(BASELINE, encoding="utf-8") as f:
        baseline = json.load(f)
    failures = []
    for path in sorted(set(current) | set(baseline)):
        now, allowed = current.get(path, 0), baseline.get(path, 0)
        if now > allowed:
            failures.append(f"{path}: {now} comment lines, baseline allows {allowed}")
    if failures:
        print("comment ratchet: new comments are banned (see docs/adr/ and "
              "scripts/comment-baseline.json).")
        print("Move the rationale to an ADR instead. Offending files:")
        for line in failures:
            print(f"  {line}")
        return 1
    print(f"comment ratchet ok: {sum(current.values())} comment lines "
          f"(baseline {sum(baseline.values())})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
