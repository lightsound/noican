<!-- agent-rules:begin source=base rev=72babce8f8167b888914aa151d473e453b9cc730 hash=7921fcf0e21a79ecdd148be29662f810c7fedf57094c8db2b51a3c11007cf20a -->
# Personal instructions

Portable conventions for AI coding agents. Everything here holds in any clone of any repository, including a fresh checkout on a cloud VM; nothing depends on one machine's paths or tools. Where a project-specific section of the file that carries this text says otherwise, the project-specific section takes precedence.

## Language

- English everywhere: code, comments, identifiers, commit messages, branch names, Issues, PR titles and bodies, review comments, and every file in the repository.
- Two exceptions: translation and i18n files together with user-facing UI copy, which follow the product's language; and the chat between the agent and the user, which is Japanese.

## Reporting

- No interim progress reports. Report once, when the work is done, with the results.
- Write the chat in concise, plain Japanese: prose is short, but items the rules require (the decisions table, PR URLs, test counts, and other structured facts) are never omitted or aggregated for brevity.

## Decisions

- When implementation needs a judgment call, do not ask the user. Propose a solution, then run rounds of searching for a strictly better alternative or a silver bullet; stop the search when a round produces no new option.
- Then extract the principle that generates the constraint and check whether the problem can be dissolved structurally. Only after that pick the best option.
- The final report lists every judgment call in a table with three columns: decision, chosen option, and the round in which no new option appeared. Never summarize or aggregate this table; when relaying another agent's report, keep it intact.

## Delivery

- When the work is done, open the PR as ready for review, not as a draft, and address review-bot findings.

## Instruction files

- `AGENTS.md` carries the content; `CLAUDE.md` contains exactly `@AGENTS.md`. Do not put content in `CLAUDE.md` and do not write a prose pointer ("see AGENTS.md"): Claude Code only loads the `@` import form.
- Tool-scoped rules (glob-activated) go in `.cursor/rules/*.mdc` or `.claude/rules/*.md`, not in the root pair.
- When an instruction file names a command or path, it must exist in the repository at the time of writing. Remove or update the reference when the target is renamed or deleted.
- `README.md` is for humans and marketing only. Do not create or update it unless the user explicitly asks, and never duplicate `AGENTS.md` content into it. Agent-facing instructions live in `AGENTS.md`.
<!-- agent-rules:end -->
