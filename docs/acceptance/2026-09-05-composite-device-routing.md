# Result record — Composite input/output microphone routing (Shure MV7i), 2026-09-05

Hardware run of the "Composite input/output microphone
(headphone-equipped USB microphone)" procedure of
[docs/macos-hardware-test.md](../macos-hardware-test.md) on the PR #26
build. Before this fix, recordings from the Noican virtual microphone
were completely silent whenever a microphone with output channels of its
own was selected, while the preview kept working and the log showed
neither an error nor an underrun line.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air 15-inch, 2023 — Apple M2 (same machine as the prior records) |
| macOS version | macOS Tahoe 26 |
| App commit | `cursor/fix-composite-device-virtual-mic-silence-1d7a` at `885f58e` (PR #26; the diagnostics lines quoted below are from this commit, the routing code is unchanged since `adfb9b7`) |
| Microphone | **Shure MV7i** — one USB device, `system_profiler SPAudioDataType`: Input Channels 2, Output Channels 2, Current SampleRate 48000, Transport USB. Also present on the machine: "MOTIV Mix Virtual" (Shure's virtual device, Transport Virtual, the system default input — excluded from Noican's microphone list by the virtual-transport filter) |
| Virtual output | Noican Microphone (`lightsound`, in 2 / out 2, 48 kHz) |
| Transport | Aggregate (`[MV7i, Noican]`, MV7i clock master) |
| Model | Owner's selection; not material to the routing check |

## Measured (verbatim log, `log stream --predicate 'subsystem == "com.lightsound.noican"' --level info`)

```
13:32:37.262 Aggregate composed: microphone "Shure MV7i" (in 2 / out 2), virtual output "Noican Microphone" (out 2); aggregate reports 4 output channel(s) in 2 stream(s)
13:32:38.454 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false
13:32:38.454 Aggregate output routing: aggregate output channels 4, virtual output at channels 2..4, channel map requested [-1, -1, 0, -1], read back [-1, -1, 0, -1]
```

What the two lines establish:

- Core Audio composes the private aggregate with **4 output channels in
  2 streams**: the MV7i's two headphone channels first, the virtual
  output's two channels after them — the layout the diagnosis
  predicted. AUHAL's default identity map (client 0 → device 0) therefore
  drove the MV7i's headphone output, not the virtual output.
- AUHAL's element-0 output-scope format reports the aggregate's
  **total** channel count (4, not the first stream's 2) — the one API
  fact no primary source stated, now observed. The requested map lands
  the mono engine signal on device channel 2 (the virtual output's first
  channel) and the read-back equals the request.

## Results

| # | Check | Result |
|---|---|---|
| 1 | MV7i → On → QuickTime recording from "Noican Microphone" carries the processed speech | **Pass** — recording is non-silent (the pre-fix defect was a completely silent file) |
| 2 | Nothing from Noican on headphones plugged into the MV7i's own jack (On and Preview) | Not exercised in this run — see "Not covered" |
| 3 | Built-in microphone recording still carries audio | **Pass** — recorded normally on the same build |
| 4 | Preview, model switching, meters, underrun diagnostics unchanged with the composite device | Reported as unchanged by the owner on the day, but on a process that turned out to be the pre-fix binary (see below); **not re-run on the fixed build** |
| 5 | Split path (Bluetooth headset) recording carries audio | **Pass** — recorded normally (the split transport takes no channel map; exercised on the pre-fix process, whose split path is identical) |
| — | Routing refusal (`virtual output routing failed`) | Did not occur |

### Not covered

- Criterion 2 (the negative check on the MV7i's own headphone jack) and
  the per-channel level pin of criterion 3 (channel 0 level within ±1 dB
  of the previous build, channel 1 silent) were not measured; criterion
  3 is "carries audio" only. On the built-in-microphone layout the map
  is byte-identical to AUHAL's default, so no level change is possible
  by construction; the measurement remains the procedure's requirement
  for a full record.
- Criterion 4 on the fixed build.

### Procedure lesson: relaunch after rebuilding

The first attempt on this build reported silence with the MV7i and no
routing refusal. The log showed the same PID (`44414`) as the earlier,
pre-fix session: `scripts/build-macos-app.sh` replaces
`dist/Noican.app` on disk but does not touch a running instance, so the
test had exercised the old binary. After `pkill -x NoicanMenuBar` and
`open dist/Noican.app` (new PID `70688`) the two new diagnostics lines
appeared and the recording carried audio. The hardware test plan now
tells the operator to quit and relaunch after every build and to confirm
the PID in the log changed.

A second, unrelated stumble on the same day: SwiftPM did not pick up the
new `NoicanState` source file from a stale `macos/.build`, failing the
app build with "cannot find type 'VirtualOutputChannels' in scope";
`rm -rf macos/.build macos/NoicanState/.build` fixed it.
