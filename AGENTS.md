<!-- agent-rules:begin source=base rev=749176a6369bb4206fc0ffb6e49329cf01c72040 hash=0ba521f2f137b14e657ffd3de0f4dc764d5a943199c3fd62f6e307f8d64cfbe1 -->
# Shared conventions

Portable conventions for AI coding agents. Everything here holds in any clone of any repository, including a fresh checkout on a cloud VM; nothing depends on one machine's paths or tools. Where a project-specific section of the file that carries this text says otherwise, the project-specific section takes precedence.

## Language

- English everywhere in the repository: code, comments, identifiers, commit messages, branch names, Issues, PR titles and bodies, and review comments.
- Exception: translation and i18n files and user-facing UI copy follow the product's language.

## Instruction files

- `AGENTS.md` carries the content; `CLAUDE.md` contains exactly `@AGENTS.md`. Do not put content in `CLAUDE.md` and do not write a prose pointer ("see AGENTS.md"): Claude Code only loads the `@` import form.
- Tool-scoped rules (glob-activated) go in `.cursor/rules/*.mdc` or `.claude/rules/*.md`, not in the root pair.
- When an instruction file names a command or path, it must exist in the repository at the time of writing. Remove or update the reference when the target is renamed or deleted.
- Agent-facing instructions live in `AGENTS.md`. `README.md` is for humans; never duplicate `AGENTS.md` content into it.
<!-- agent-rules:end -->

<!-- agent-rules:begin source=personal rev=749176a6369bb4206fc0ffb6e49329cf01c72040 hash=f1a5d3d27bb791dd3b47db3d20abb2766ca3b475178c259a8388ede569050e30 -->
# Personal working style

How this repository's owner works with agents. Portable: nothing here depends on one machine or repository. Where a project-specific section of the file that carries this text says otherwise, the project-specific section takes precedence.

## Chat and reporting

- The chat between the agent and the user is Japanese.
- No interim progress reports. Report once, when the work is done, with the results.
- Prose is concise and plain, but items the rules require (the decisions table, PR URLs, test counts, and other structured facts) are never omitted or aggregated for brevity.

## Decisions

- When implementation needs a judgment call, do not ask the user. Propose a solution, then run rounds of searching for a strictly better alternative or a silver bullet; stop the search when a round produces no new option.
- Then extract the principle that generates the constraint and check whether the problem can be dissolved structurally. Only after that pick the best option.
- The final report lists every judgment call in a table with three columns: decision, chosen option, and the round in which no new option appeared. Never summarize or aggregate this table; when relaying another agent's report, keep it intact.

## Delivery

- When the work is done, open the PR as ready for review, not as a draft, and address review-bot findings.
- Do not create or update `README.md` unless the user explicitly asks.
<!-- agent-rules:end -->

# Noican

Fully on-device noise-cancelling virtual microphone for macOS: a Rust audio engine (`crates/`), a SwiftUI menu-bar app (`macos/`), and the separate GPL-3.0 driver consumed as the `external/noican-driver` submodule.

## Where things are documented

This repository has no `README.md` by owner decision: it is developed only by agents, and `AGENTS.md` plus `docs/` are the documentation. Each fact has one owner; link to it instead of restating it.

- Architecture, roadmap, and phase status: `docs/tech-research.md` (§12).
- Quality gates: `.github/workflows/ci.yml` is the source of truth; the policy behind it is `docs/tech-research.md` §12 "Cross-cutting: quality gates".
- Crate responsibilities: the `//!` docs at the top of each crate's `lib.rs` or `main.rs`; workspace members are listed in `Cargo.toml`.
- CLI (`noican models` / `fetch` / `process`) and supported models: `docs/models.md`; `noican eval`: `docs/hush-48k-eval.md`; accepted input formats: `crates/noican-cli/src/audio.rs`.
- macOS app build, prerequisites, and hardware acceptance: `docs/hardware-test/setup.md` and `docs/macos-hardware-test.md`; each run is recorded under `docs/acceptance/`.
- Driver: `docs/driver.md`. License activation: `docs/licensing.md`. Release checklist: `docs/release.md`.
- Testing on a Mac VM: `.agents/skills/testing-noican/SKILL.md`.
