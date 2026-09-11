# macOS Build and Hardware Test Plan

## Verification boundary

The Linux CI covers the common engine, real ONNX inference, multi-format CLI
input (WAV/AIFF/AIFC/CAF/M4A), lock-free model switching, the native-rate
input resampler and clock-drift servo (per-ratio delay reporting, drift
cancellation, strength alignment through the resampler), and the C ABI. The
macOS CI job additionally covers clippy/tests for `aarch64-apple-darwin`
(including the AUHAL transport), SwiftLint in strict mode, the release
app build with `swift -warnings-as-errors`, and an ad-hoc build of the
Noican driver (compile check only — coreaudiod will not load an ad-hoc
signature; see docs/driver.md).

Physical microphone capture, TCC prompts, aggregate clock behavior,
BlackHole routing, audible switching, 16 kHz-model audio quality, and
long-session stability require Apple hardware. So do the Phase 2
controls: login-item registration (`SMAppService` depends on the app's
location and signature — CI only compiles that path), the audible
quality of the dry/wet strength mix, and preference restoration across
real relaunches. **They are not claimed as verified until every
applicable check below is recorded.** The transport design passed this
plan on macOS 26 / Apple Silicon in its candidate-B incarnation; the
hybrid build (C engine + B transport) must be re-accepted.

## Procedures and acceptance checklists

The procedures and the checklists that score them live in one file per
subject under [hardware-test/](hardware-test/); the table lists them in
the order of the original plan, which is also a sensible run order (build
and driver first, endurance last). Each file keeps the procedure and,
where one exists, the acceptance checklist that scores it, so a record
can cite "checklist name + criterion number" exactly as before — the
checklist headings and criterion numbers are unchanged from the
single-file plan that this document was until 2026-09-11. Results are
recorded under [acceptance/](acceptance/) following the rules in
"Result record" below; records written before the split cite this file
as a whole and resolve here.

| File | Procedures | Acceptance checklist |
|---|---|---|
| [setup.md](hardware-test/setup.md) | Prerequisites · Build · Model weights | — (the Build section's expected results are scored directly, e.g. [acceptance/2026-09-09-developer-id-microphone.md](acceptance/2026-09-09-developer-id-microphone.md)) |
| [driver-check.md](hardware-test/driver-check.md) | Driver check | Acceptance checklist (1-channel driver) |
| [functional-test.md](hardware-test/functional-test.md) | Functional test · Model switching · Microphone switching | Acceptance checklist (Phase 0 hybrid build) |
| [native-rate-capture.md](hardware-test/native-rate-capture.md) | Non-48 kHz microphone (native-rate capture) | Acceptance checklist (native-rate capture, issue #7) |
| [composite-microphone.md](hardware-test/composite-microphone.md) | Composite input/output microphone (headphone-equipped USB microphone) | Acceptance checklist (composite input/output microphone) |
| [level-integrity.md](hardware-test/level-integrity.md) | Level integrity | Acceptance checklist (level integrity) |
| [preview-and-level-meters.md](hardware-test/preview-and-level-meters.md) | Preview (self-monitor) · Level meters | Acceptance checklist (preview + level meters) |
| [phase-2-controls.md](hardware-test/phase-2-controls.md) | Settings persistence · Launch at login · Strength control | Acceptance checklist (Phase 2 controls) |
| [real-time-audit.md](hardware-test/real-time-audit.md) | Real-time audit | — (the "callback audit" of the result record; also cited by native-rate criterion 8) |
| [output-underrun-diagnostics.md](hardware-test/output-underrun-diagnostics.md) | Output-underrun diagnostics (real-time budget) | Acceptance checklist (output-underrun diagnostics) |
| [clock-drift-and-endurance.md](hardware-test/clock-drift-and-endurance.md) | Clock drift and endurance | — (the "endurance" of the result record) |

## Result record

For each run, retain:

- Mac model and chip,
- macOS and Xcode versions,
- app and driver commit IDs,
- app and driver signature output,
- physical input and virtual device UIDs,
- selected model,
- sample rate and buffer size,
- test recording,
- pass/fail for the acceptance checklist, callback audit, and endurance,
- every unverified or failed item.

Hardware acceptance is complete only when this record contains evidence for
every check rather than an inferred result.
