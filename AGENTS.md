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
