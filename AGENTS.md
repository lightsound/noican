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
