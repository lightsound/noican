# Result record — Dual-mono aggregate routing and level integrity, 2026-09-05

Hardware run of the "Composite input/output microphone" (dual-mono
criteria) and "Level integrity" procedures of
[docs/macos-hardware-test.md](../macos-hardware-test.md) on the PR #27
build (merged to main as `c10b929`). PR #27 replaced PR #26's
single-channel map (engine signal on the virtual output's first channel,
second channel silent — measured −16.5 dBFS / silent on the built-in
microphone) with a dual-mono render and a one-to-one channel map, and
added detection of a turned-down or muted Noican Microphone device.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air 15-inch, 2023 — Apple M2 (same machine as the prior records) |
| macOS version | macOS Tahoe 26 |
| App commit | main `c10b929` (PR #27), process PID 21081 (fresh launch after the build) |
| Microphones | Built-in ("MacBook Airのマイク", in 1 / out 0); Shure MV7i (in 2 / out 2, USB); Bluetooth headset (split transport) |
| Virtual output | Noican Microphone (in 2 / out 2, 48 kHz) |
| Engine settings | Passthrough, strength 100%, Noican Microphone slider at maximum, the microphone's own system slider at maximum unless stated |
| Measurement | QuickTime (maximum quality) → `afconvert -f WAVE -d LEI16` → the per-channel RMS/peak script from the Level integrity section |

## Measured (verbatim)

Per-channel levels of the four recordings:

```
== 01-builtin-noican.wav
channels: 2
ch0: rms  -19.6 dBFS  peak 0.918  (signal)
ch1: rms  -19.6 dBFS  peak 0.918  (signal)
== 02-mv7i-noican.wav
channels: 2
ch0: rms  -26.7 dBFS  peak 0.371  (signal)
ch1: rms  -26.7 dBFS  peak 0.371  (signal)
== 03-bt-mid.wav            (headset system slider at the middle)
channels: 2
ch0: rms  -23.2 dBFS  peak 0.595  (signal)
ch1: rms  -23.2 dBFS  peak 0.595  (signal)
== 04-bt-max.wav            (headset system slider at maximum)
channels: 2
ch0: rms  -21.0 dBFS  peak 0.882  (signal)
ch1: rms  -21.0 dBFS  peak 0.882  (signal)
```

Unified log, `log stream --predicate 'subsystem == "com.lightsound.noican"' --level info`
(timestamps JST, PID 21081 throughout):

```
18:14:37.464 Aggregate composed: microphone "Shure MV7i" (in 2 / out 2), virtual output "Noican Microphone" (out 2); aggregate reports 4 output channel(s) in 2 stream(s)
18:14:38.597 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
18:14:38.597 Aggregate output routing: aggregate output channels 4, virtual output at channels 2..4, channel map requested [-1, -1, 0, 1], channel map read back after initialize [-1, -1, 0, 1] (device output channels then 4)
18:16:22.084 Aggregate composed: microphone "MacBook Airのマイク" (in 1 / out 0), virtual output "Noican Microphone" (out 2); aggregate reports 2 output channel(s) in 1 stream(s)
18:16:23.147 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
18:16:23.147 Aggregate output routing: aggregate output channels 2, virtual output at channels 0..2, channel map requested [0, 1], channel map read back after initialize [0, 1] (device output channels then 2)
18:16:33.568 Virtual output level: "Noican Microphone" volume scalar 0.487 (below unity) — consumers hear the engine output attenuated; not changed by Noican
18:16:40.756 Virtual output level: "Noican Microphone" volume scalar 0.686 (below unity) — consumers hear the engine output attenuated; not changed by Noican
18:16:42.774 Virtual output level: "Noican Microphone" back at unity and unmuted
18:16:44.803 Virtual output level: "Noican Microphone" volume scalar 0.921 (below unity) — consumers hear the engine output attenuated; not changed by Noican
18:16:45.819 Virtual output level: "Noican Microphone" volume scalar 0.936 (below unity) — consumers hear the engine output attenuated; not changed by Noican
18:16:46.841 Virtual output level: "Noican Microphone" back at unity and unmuted
18:17:28.952 Virtual output level: "Noican Microphone" is muted; not changed by Noican
18:17:40.416 Virtual output level: "Noican Microphone" back at unity and unmuted
18:18:06.731 Aggregate composed: microphone "Shure MV7i" (in 2 / out 2), virtual output "Noican Microphone" (out 2); aggregate reports 4 output channel(s) in 2 stream(s)
18:18:07.857 Aggregate output routing: aggregate output channels 4, virtual output at channels 2..4, channel map requested [-1, -1, 0, 1], channel map read back after initialize [-1, -1, 0, 1] (device output channels then 4)
18:19:19.278 Aggregate composed: microphone "Shure MV7i" ... (as above)
18:19:20.381 Aggregate output routing: ... [-1, -1, 0, 1] ... [-1, -1, 0, 1] (as above)
18:19:21.586 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false   (Bluetooth headset start — the split transport logs no routing line)
```

What the log establishes:

- The **two-entry one-to-one map** `[-1, -1, 0, 1]` — the shape PR #27
  builds for a composite device and the one PR #26's record had not
  observed — is accepted by AUHAL and read back unchanged **after
  `AudioUnitInitialize`** on a 4-channel aggregate, three starts in a
  row. The built-in layout's `[0, 1]` likewise.
- The level probe reads the Noican Microphone device's volume scalar
  (0.487 for a slider at about 50%), follows a drag one reading per
  second (0.686, 0.921, 0.936), logs the return to unity, and reports
  mute and unmute. The level was never written by the app (the operator
  moved it back by hand).

## Results

| # | Check | Result |
|---|---|---|
| 1 | Dual mono on the built-in microphone: same signal on every channel (RMS within 0.1 dB) | **Pass** — ch0 = ch1 = −19.6 dBFS, identical peaks |
| 1 | Built-in microphone channel 0 within ±1 dB of the previous build | **Pass (operator judgement)** — no previous-build recording existed on the day (the old bundle had been deleted), so the comparison is against the 2026-09-05 pre-PR figure of −16.5 dBFS. The new reading, −19.6 dBFS, is 3.1 dB lower; the operator attributes the difference to speaking level (single-take recordings ten minutes apart, different sentences). Consistent with the code, which writes the same sample values as before into every channel and touches no gain. See "Not covered" for the same-session A/B that would pin this |
| 2 | Dual mono on a composite device (MV7i), one-to-one map read back | **Pass** — ch0 = ch1 = −26.7 dBFS; `channel map requested [-1, -1, 0, 1]`, `read back after initialize [-1, -1, 0, 1]`, `device output channels then 4` |
| 3 | Both ears | **Pass** — reported by the operator (Preview and playback) |
| 4 | Turned-down / muted notice: shown within a second at 50%, cleared at maximum; mute likewise; log carries the scalar; level never changed by Noican | **Pass** — operator confirmed the popover notice appearing and clearing; log lines above (scalar 0.487; mute at 18:17:28, cleared 18:17:40) |
| 5 | Preview, model switching as before | **Pass** — Preview audible, a model switch clean (operator report) |
| 6 | Split path: Bluetooth recording carries both channels at the usual level | **Pass** — ch0 = ch1 on both takes |
| 6 | Split path: does the headset's system input slider apply? | **Measured, operator-judged "not applied"** — middle −23.2 dBFS, maximum −21.0 dBFS (+2.2 dB RMS, +3.4 dB peak). The direct-recording reference for an applied slider is +7.6 dB (built-in microphone, PR #27 investigation). The operator attributes the 2.2 dB to speaking level. Recorded as such in the Level integrity section; one pair only |
| — | Routing refusal (`virtual output routing failed`) | Did not occur on any of the four starts |

### Not covered

- **Same-session previous-build A/B** for the level pin (criterion 1,
  second row): the previous build was not available; the comparison is
  against a figure recorded earlier the same day under nominally equal
  settings. A rebuild of `9d521b3` and one more built-in-microphone
  take would close it properly.
- **Underrun line absence** on a light model over 60 s of speech was
  not specifically watched on this build (no `Output underruns` line
  appears in the captured log, but the log excerpt does not cover a
  timed 60 s FastEnhancer-B session).
- **The MV7i's own headphone jack** (nothing from Noican audible there)
  — the negative check from the composite-microphone procedure, still
  unexercised across both records.
- **Second Bluetooth slider pair** to firm up the +2.2 dB reading.
- The exact **latency** of the notice (the "within one second" claim)
  was judged by eye, not timed; the log shows the poll reacting within
  its 1 Hz cadence.

### Observations

- Log wording: the operator found `volume scalar 0.487` hard to read as
  a slider position; the line now also prints the percentage (changed
  in the PR that adds this record). The popover text itself was judged
  clear.
- While the slider is dragged, one line per second is written for each
  new value (four lines for one drag here) — intended, and the reason
  the log dedupes on the reading rather than the notice text.
