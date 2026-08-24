# noican

A personal, fully on-device noise-cancelling virtual microphone for macOS, in the spirit of [JoyCast](https://joycast.ai/).

The physical microphone signal is captured, cleaned in real time (noise suppression and, eventually, background-speaker suppression), and re-exposed to the system as a virtual input device that any app (Zoom, Meet, Teams, OBS, Discord, ...) can select as its microphone. Everything runs locally on Apple Silicon; no audio ever leaves the machine.

## Status

Design / research phase. No implementation yet.

- [docs/tech-research.md](docs/tech-research.md) — consolidated technology research: candidate evaluation for every layer, final recommended stack, roadmap, and open questions.

## Scope

- macOS only (Apple Silicon first), personal use only.
- No distribution, licensing, or multi-user concerns; a paid Apple Developer Program membership is available for Developer ID signing.
- Target: ~20–30 ms end-to-end latency, < 100 MB memory, 48 kHz native audio path.
