# noican

A personal, fully on-device noise-cancelling virtual microphone for macOS, in the spirit of [JoyCast](https://joycast.ai/).

The physical microphone signal is captured, cleaned in real time (noise suppression and, eventually, background-speaker suppression), and re-exposed to the system as a virtual input device that any app (Zoom, Meet, Teams, OBS, Discord, ...) can select as its microphone. Everything runs locally on Apple Silicon; no audio ever leaves the machine.

## Status

Design / research phase. No implementation yet.

- [docs/tech-research.md](docs/tech-research.md) — consolidated technology research: candidate evaluation for every layer, final recommended stack, roadmap, and open questions.

## Scope

- macOS only (Apple Silicon first). Built for personal use first, with a possible future sale in mind (free/open stack only; publishing source is acceptable).
- A paid Apple Developer Program membership is available for Developer ID signing.
- Target: ~20–30 ms end-to-end latency, < 100 MB memory, 48 kHz native audio path.

## Development

### Lint policy

Two TS/JS quality gates are wired in from day one so they apply the moment any TypeScript/JavaScript enters the repository (the core is planned in Rust + SwiftUI; Rust-side equivalents like `cargo clippy -- -D warnings` will be added with the Rust workspace):

- **[fallow](https://github.com/fallow-rs/fallow)** (`npm run lint:fallow`) — dead code, circular dependencies, duplication, complexity, boundaries. **Every rule is set to `error`** in `.fallowrc.json`; a rule may only be demoted/disabled there with a written reason (currently only `coverage-gaps`, which requires the paid Fallow Runtime). Inline suppressions must carry a `-- <reason>` suffix (enforced by `require-suppression-reason`).
- **[ImportLint](https://github.com/uhyo/import-lint)** (`npm run lint:imports`) — directory-level encapsulation: a `*.package` directory's exports are importable from outside only when tagged `@public`. Config in `.importlintrc.jsonc`, severity `error`.

Both run in CI on every push/PR (`.github/workflows/lint.yml`). Run everything locally with `npm run lint`.
