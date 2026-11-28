---
name: delete-comment
description: Audit every code comment in crates/ and macos/, classify it as Tautology / Excuse / Stale / Decision Record, delete or relocate it accordingly, and lower the comment ratchet baseline
---

# Deleting comments (the comment audit)

Code comments in `crates/` and `macos/` are banned in principle. Rationale
belongs in `docs/adr/`; the CI comment ratchet (`scripts/comment-ratchet.py`,
baseline `scripts/comment-baseline.json`) fails any change that raises a
file's comment-line count. This skill is the repeatable pass that drives the
remaining counts down. Run it when the baseline has accumulated entries worth
another look, or after a large feature lands with new comments.

Exempt from everything below: `// swift-tools-version:` lines in
`Package.swift` files (a SwiftPM directive in comment syntax, not a comment).

## 1. Inventory

Extract every comment block from `.rs` and `.swift` files under `crates/`
and `macos/` (skip `.git`, `target`, `.build`):

- Scan string-aware: a `//` inside a `"..."` literal is not a comment.
- Group consecutive whole-line `//` comments into one block; a trailing
  comment (code before it on the line) is its own single-line block.
- Record per block: file, start/end lines, kind (`///`/`//!` doc vs `//`
  line), and the full text. Note the totals: blocks, comment lines, and the
  share of total source lines — the report needs them.

## 2. Classify every block

- **Tautology** — restates what the code already says (identifier expansions,
  test narration that paraphrases the assertion below, `// MARK:` banners).
  Test: deleting it loses no information a reader cannot recover from the
  code.
- **Stale** — references code, behaviour, or state that no longer exists,
  including commented-out code.
- **Excuse** — gives permission for a hacky or unfinished implementation
  ("for now", "good enough", an unvalidated value shipped as final). Test
  against Decision Record: does the comment state a constraint that would
  still bind a rewrite (Decision Record), or excuse not doing the work
  (Excuse)?
- **Decision Record** — documents a non-obvious decision, contract, or
  measured fact a rewrite must preserve (invariants invisible from
  signatures, rejected alternatives, hardware measurements, security
  bounds). Give each one a short kebab-case topic slug for ADR grouping.
- Doc comments are judged by the same standard as line comments.
- Escape hatch: mark extractor false positives (e.g. a directive line) as
  not-a-comment and leave them alone.

## 3. Act on each class

- **Tautology / Stale** → delete the block.
- **Excuse** → delete the block, and file a GitHub issue that names the
  hacky implementation and the work that removes it. Group blocks excusing
  the same hack into one issue.
- **Decision Record** → move the text into `docs/adr/`, grouped by topic
  into the existing numbered ADRs (add a new ADR only for a genuinely new
  topic). Preserve the original wording; note above its text what the block
  was attached to, as ``Source: `path`, `Symbol`.`` (or ``Source: `path`.``
  for a module doc). Line numbers are not a source: they stop being true at
  the next edit. `python3 scripts/adr-links.py --symbols <file>` lists the
  symbols a file defines. Then delete the block from the code.

Two constraints shape the deletions:

- The workspace lints set `missing_docs = "deny"`. Whether a doc comment is
  forced is a property of the item's *effective* visibility, not of the
  keyword on its own line: enum variants and fields, trait items, impl
  methods of public types, and the `//!` doc of a `pub mod` module are all
  forced even though they carry no `pub` of their own, while `pub(crate)` /
  `pub(super)` items and `pub` items in private modules are not forced.
  Decide with the compiler, not by line shape: strip the docs in a scratch
  copy and read the `missing documentation` errors, per crate and per
  target (a dependency's failure hides its dependents, and `cfg`-gated
  modules answer only on their own target).
- For a Decision Record in a forced position (or a `//!` module doc), leave
  a pointer instead: `/// See docs/adr/NNNN-<topic>.md#<section>.`, where
  `<section>` is the record's `## ` heading. One exception:
  clippy's `missing_errors_doc` / `missing_panics_doc` /
  `missing_safety_doc` require a public function's doc to keep its
  `# Errors` / `# Panics` / `# Safety` section, so retain those sections
  verbatim under the pointer line. A Tautology doc in a
  forced position stays for now — it is carried by the ratchet baseline
  and reported as such.
- Deleting a block must not leave doubled blank lines behind; collapse the
  seam to one blank line. A deletion at end of file must preserve the
  file's trailing-newline state exactly (SwiftLint's `trailing_newline`
  is strict in this repository).

## 4. Verify no code changed

Run `cargo fmt` first: deleting a field's doc comment can make rustfmt
collapse the enclosing variant/struct onto one line (dropping a trailing
comma), and CI's fmt job will demand exactly that form. Then strip all
comments from the tree before and after the pass (same extractor) and
compare the remaining code as token streams (whitespace and those trailing
commas normalised): they must be identical. Comment edits are the one
change class where "the diff is only comments" can be proven mechanically
— prove it.

## 5. Lower the ratchet and finish

1. Regenerate the baseline: `python3 scripts/comment-ratchet.py --update`.
   The baseline may only shrink; if a count went up, the pass went wrong.
2. Run `python3 scripts/adr-links.py`: every pointer left in the code and
   every `Source:` line written into an ADR must resolve.
3. If ADRs were added or the documentation map changed, update the
   "Where things are documented" section of `AGENTS.md` to stay true.
4. Follow the repo's standing rules for the change itself: independent
   review loop before the PR, CI green, and a final report stating the
   category counts, the issues filed, the ADRs touched, the lines shed,
   and what remains in the baseline and why.
