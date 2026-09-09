# Result record — Split transport render format follows the virtual output, 2026-09-09

Hardware run for PR #29 (branch head `04157ad`), which sizes the split
transport's output render format from the virtual output's channel
count instead of a fixed two channels, and logs the result as
`Split output routing: …`. The run is a **regression check on the
current 2-channel driver (0.1.0)**: the render format must come out at
2 ch, both channels must carry the same signal at the usual level, and
the aggregate path must be unaffected. It scores native-rate checklist
criterion 10 (added by PR #29) and level-integrity criterion 6 of
[docs/macos-hardware-test.md](../macos-hardware-test.md). The 1-channel
driver (PR #30) is a separate, later run.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air — Apple M2, 24 GB |
| macOS / Xcode | macOS 26.6.2 (25G83) / Xcode 26.6 (17F113) |
| App commit | `04157ad` (PR #29 head); built with `scripts/build-macos-app.sh`, Rust staticlib release, Swift release |
| App signature | Ad-hoc (see "Observations" — the Developer ID build of the same binary was unusable for microphone capture) |
| App process | PID 28370 for every measurement below (fresh launch after re-signing) |
| Driver | Noican.driver `0.1.0` (2 channels, `com.lightsound.noican.2ch_UID`), Developer ID |
| Microphones | HUAWEI FreeClip (Bluetooth, split transport); built-in ("MacBook Airのマイク", in 1 / out 0, aggregate path) |
| Virtual output | Noican Microphone (in 2 / out 2, 48 kHz) |
| Engine settings | Passthrough, strength 100%, Noican Microphone slider at maximum, the microphone's own system slider at maximum |
| Measurement | QuickTime (maximum quality) → `afconvert -f WAVE -d LEI16` → the per-channel RMS/peak script from the Level integrity section |

## Measured (verbatim)

```
== pr29-bt-max.m4a          (HUAWEI FreeClip, split transport, 30 s)
channels: 2
ch0: rms  -23.5 dBFS  peak 0.821  (signal)
ch1: rms  -23.5 dBFS  peak 0.821  (signal)

== pr29-builtin-max.m4a     (built-in microphone, aggregate path, 30 s)
channels: 2
ch0: rms  -20.1 dBFS  peak 0.890  (signal)
ch1: rms  -20.1 dBFS  peak 0.890  (signal)
```

Container format of the two takes (`afinfo`):

```
pr29-bt-max.m4a       Data format: 2 ch, 48000 Hz, aac   estimated duration: 31.09 s
pr29-builtin-max.m4a  Data format: 2 ch, 48000 Hz, aac   estimated duration: 30.94 s
```

Operator's note on the takes: the same sustained sound for 30 s, with a
breath at about 20 s in both, and faint household sounds (another
person, a child) possibly present in the background.

Unified log (`log show --info --predicate 'subsystem ==
"com.lightsound.noican"'`, timestamps JST, PID 28370):

```
15:45:29.623 Aggregate composed: microphone "MacBook Airのマイク" (in 1 / out 0), virtual output "Noican Microphone" (out 2); aggregate reports 2 output channel(s) in 1 stream(s)
15:45:30.691 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
15:45:30.691 Aggregate output routing: aggregate output channels 2, virtual output at channels 0..2, channel map requested [0, 1], channel map read back after initialize [0, 1] (device output channels then 2)
15:52:51.099 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
15:52:51.099 Split output routing: virtual output channels 2, render format requested 2 ch, render format read back after initialize 2 ch
```

The 15:52 pair is a Bluetooth start made after the recordings, on the
operator's request, because the Bluetooth session that produced
`pr29-bt-max` (about 15:44) had already been purged from the unified
log when it was queried — Info-level messages are held in memory and
dropped after a short while. The same build, signed Developer ID and
running as PID 26092, had logged the identical line at 14:55:20 and
14:56:36 and 14:56:44 (three Bluetooth starts) and the identical
`Aggregate output routing` line four times between 14:56 and 14:57;
those were read at the time and match the lines above word for word.

What the log establishes:

- **The device-side stream-format read on an output-only AUHAL returns
  the virtual output's channel count** (2 here), and the client format
  set from it reads back unchanged after `AudioUnitInitialize` — the
  one Core Audio behaviour the render-format decision record
  (`noican_coreaudio::routing`, "The split transport's render format")
  rested on without prior hardware evidence. Observed on four
  Bluetooth starts across two processes.
- The aggregate path is unchanged: `[0, 1]` requested and read back,
  aggregate output channels 2.
- No `virtual output routing failed` refusal and no underrun line
  appeared in any of the captured windows.

## Results

Native-rate checklist (docs/macos-hardware-test.md):

| # | Check | Result |
|---|---|---|
| 1 | Running on a native rate (Bluetooth headset) | **Pass** — HUAWEI FreeClip, engine Running, 30 s recording taken |
| 2 | 48 kHz output, intelligible | **Pass** — `afinfo`: 48000 Hz, 31.1 s; intelligibility by the operator's own listening (telephony bandwidth expected on this HFP headset, not separately auditioned by a second listener) |
| 4 | Aggregate path unchanged (behavior, quality, latency) | **Partial** — behavior: built-in microphone start, `[0, 1]` / `[0, 1]`, ch0 = ch1 = −20.1 dBFS (−0.5 dB against 2026-09-05). Quality and latency were not compared against a pre-PR build in this run (same evidence tier as the 2026-09-05 split-transport record's Partial) |
| 5 | Strength alignment (single voice at 50%) | Not covered (Passthrough at 100% only) |
| 10 | Split render format follows the device: `Split output routing` counts equal the device's channel count, requested = read-back, same signal on every channel | **Pass** — `virtual output channels 2, render format requested 2 ch, render format read back after initialize 2 ch`; Noican Microphone is a 2-channel device; ch0 = ch1 (identical RMS and peak) |
| 3, 6–9 | Drift/endurance, rate-change recovery, UI truthfulness, real-time audit, split-path underruns | Not covered — outside this regression check; the code paths for the callback are unchanged by PR #29 and the earlier split-transport records stand |

Level-integrity checklist:

| # | Check | Result |
|---|---|---|
| 6 | Split path level: Bluetooth recording unchanged (every channel fed, same level as before) | **Pass (operator/agent judgement)** — ch0 = ch1 = −23.5 dBFS against the 2026-09-05 `04-bt-max` reference of −21.0 dBFS: −2.5 dB, outside the ±1.5 dB speech-variance band. Judged a level-of-speech difference, not a routing one: the two channels are identical to the sample (the only thing PR #29 changes is the channel count of the client format, not the sample values or any gain), the same headset's two 2026-09-05 takes were already 2.2 dB apart under nominally equal settings, and this take contains a breath pause and background sounds. A same-session A/B against `main` was not run (see "Not covered") |
| 6 | Split path: does the headset's system input slider apply? (middle / maximum pair) | **Not covered** — only the maximum-slider take was recorded; the 2026-09-05 pair (+2.2 dB, judged "not applied") remains the only evidence, and the Level integrity section's second bullet is unchanged |
| 1 | Aggregate path level (built-in microphone): same signal on every channel, channel 0 within ±1 dB of the previous build | **Pass** — ch0 = ch1 = −20.1 dBFS against −19.6 dBFS on 2026-09-05 (−0.5 dB) |
| 5 | Preview, model switching, underrun diagnostics as before | **Partial** — Preview audible on the headset and one model switch clean on the split path (operator report). The underrun clause (zero on a light model over 60 s) was not run; no underrun line appeared in the captured windows, but those were not a timed light-model session |
| 2, 3, 4 | Dual mono on a composite device; both ears; turned-down / muted notice | Not covered — no composite device was connected (the MV7i was not part of this run) and the level notice was not exercised; both stand on the 2026-09-05 record |

### Not covered

- **Same-session A/B against `main`** for the Bluetooth level (a
  rebuild of `728ffe5` and one more FreeClip take under identical
  settings) — would turn the −2.5 dB judgement into a measurement.
- **Stock BlackHole 2ch as the virtual output** on the split path
  (procedure step 3 of PR #29's hardware list): not exercised; the
  Noican driver stayed installed throughout.
- **44.1 kHz-family microphone** on the split path: only the 16 kHz
  HFP headset was used.
- **Headset system-slider pair** (level-integrity criterion 6, second
  row): not repeated; still one measured pair (2026-09-05).
- Native-rate criteria 3, 5, 6, 7, 8, 9; quality/latency halves of 4;
  level-integrity criteria 2, 3, 4 and the underrun clause of 5 (see
  tables).

### Observations

- **Developer ID app build cannot capture the microphone.**
  `scripts/build-macos-app.sh` signs the Developer ID variant with
  `--options runtime` (hardened runtime) and no entitlements file; the
  repository has none. `tccd` then refuses microphone access without a
  prompt: `Prompting policy for hardened runtime; service:
  kTCCServiceMicrophone requires entitlement
  com.apple.security.device.audio-input but it is missing` and `Failed
  to match existing code requirement for subject com.lightsound.noican`.
  The engine starts and logs normally (the routing lines at 14:55–14:57
  above came from that process, PID 26092), the meters just never move
  — the symptom the operator reported as "Noican does not react to my
  voice at all". Re-signing the same bundle ad-hoc (`codesign --force
  --sign -`, no hardened runtime) restored capture — the recordings
  above were made with that re-signed bundle. This is independent
  of PR #29 and predates it; the ad-hoc path is the one the hardware
  procedure documents for local builds. Fix candidates: add an
  entitlements plist with `com.apple.security.device.audio-input` to
  the Developer ID signing step, or drop `--options runtime` for the
  app (the driver's hardened-runtime signing is unaffected — it has no
  TCC-guarded capability).
- **Unified-log retention.** Info-level lines from a session that ended
  about ten minutes earlier were no longer returned by `log show`; the
  `Split output routing` evidence for the recording session had to be
  reproduced with a fresh Bluetooth start. For future runs, keep `log
  stream … --level info` writing to a file for the whole session (the
  procedure's command) rather than querying afterwards.
- In `zsh`, `log` is a shell builtin; the procedure's commands need
  `/usr/bin/log` when pasted into a zsh terminal.
