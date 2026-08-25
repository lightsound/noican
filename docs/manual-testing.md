# Manual testing on a Mac

Everything below needs hardware. None of it has been run.

What CI does establish, on a macOS runner: the Core Audio code compiles and
lints with the platform paths actually enabled, the Swift app compiles, the two
link together against ONNX Runtime, the bundle is assembled and signed, and its
`Info.plist` carries `LSUIElement` and `NSMicrophoneUsageDescription`. That is
all it can establish — the runner has no audio devices, no microphone
permission, and no way to hear anything. **Treat every step below as unverified
until you have done it.**

Work through the steps in order; each one narrows down where a failure is.

## Prerequisites

- macOS 14 or later on Apple Silicon.
- A virtual audio device with at least two output channels. Until the renamed
  fork exists (Phase 1), stock [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole)
  works: `brew install blackhole-2ch`, then log out and back in.
- Headphones. There is no echo canceller yet, so speaker playback will be
  captured back through the microphone (`docs/tech-research.md` §7).
- Model weights: `cargo run -p noican-cli -- fetch`.

## 1. Confirm the engine works before involving Core Audio

```bash
cargo run --release -p noican-cli -- process some-recording.wav
open out/some-recording/
```

Listen to `00-reference-unprocessed.wav` against each model. If this sounds
wrong, the problem is in the engine or the models, not in the audio plumbing,
and nothing below will help.

**Record the comparison material at 48 kHz.** A 48 kHz model fed upsampled
16 kHz audio attenuates the speech band by 4–16 dB for reasons that have
nothing to do with how it will sound live (`docs/tech-research.md` §5.5).

## 2. Build and launch the app

```bash
./apps/NoicanMenuBar/build.sh release
open apps/NoicanMenuBar/.build/NoicanMenuBar.app
```

A microphone icon should appear in the menu bar with no Dock icon. If a Dock
icon appears, `LSUIElement` did not make it into the bundle.

Run it from a terminal instead to see the log:

```bash
NOICAN_LOG=debug apps/NoicanMenuBar/.build/NoicanMenuBar.app/Contents/MacOS/NoicanMenuBar
```

## 3. Check the device lists

Open the menu. Expect:

- **Microphone** listing every input device, with the system default preselected.
- **Virtual output** preselected to BlackHole. The suggestion is made from the
  HAL's transport type rather than the device name, so a renamed fork will still
  be found — worth confirming, since name matching is only a tie-breaker.
- **Model** listing eleven entries, with anything not downloaded marked as such.

If the virtual output list is empty, BlackHole is not installed or
`coreaudiod` has not picked it up. `sudo killall coreaudiod` and reopen the menu.

## 4. First run: the microphone permission prompt

Toggle the switch on. macOS should prompt for microphone access **the first
time**.

If no prompt appears and the input meter stays flat, that is the known failure
mode where Core Audio hands the callback silence without reporting an error.
Check `System Settings → Privacy & Security → Microphone`.

## 5. Confirm audio is flowing

With the toggle on and someone speaking:

- The **In** meter should move.
- The **Out** meter should move, a little behind the input.
- The latency readout should show roughly what the model's entry in
  `docs/models.md` predicts, plus a few milliseconds of device buffering.
- **Dropouts should stay at zero.** A climbing count means the inference thread
  is not keeping up; try FastEnhancer T, which is the cheapest model.

Then verify the other end: open QuickTime Player → File → New Audio Recording,
select BlackHole as the input, and record. You should hear your cleaned voice.

## 6. Confirm the cleaning is real

Play something noisy near the microphone — a fan, typing, a vacuum — while
speaking. Toggle **Bypass** on and off. Bypass keeps the model running and only
discards its output, so the delay does not change and the difference you hear is
the model alone.

## 7. Switch models while running

Change the **Model** picker while audio is flowing. Expect a brief dip rather
than a click: the engine fades out, stays silent for as long as the incoming
model needs to fill its pipeline, and fades back in. The dip is roughly
30 ms + the new model's delay + 30 ms, so switching to DPDFNet-2 48 kHz HR
(50 ms of delay) should be noticeably longer than switching between the
FastEnhancer variants (10.7 ms).

**Listen specifically for a click at either end of the dip.** The ramp is unit
tested, but the interaction between the ramp and a real model's priming is not,
and it is the part most likely to be subtly wrong.

## 8. The speaker gate, which needs enrolling first

Two of the fourteen models carry conditions the others do not.

`speaker-gate` needs to know whose voice to keep. Enrol before selecting it, or
the picker will show "no profile ...; run `noican enroll` first" — which is the
intended message, not a bug:

```sh
noican enroll me-talking.wav        # 10-20 s of just you, several files is better
```

Then select it and have somebody else talk while you stay quiet. Expect your own
voice to pass untouched and theirs to drop by about 24 dB — but not instantly.
The gate needs about 1.5 s of speech to recognise anyone, so it suppresses a
sustained other voice and will not catch a single interjected word
(`docs/tech-research.md` §6.4). **This is the part most likely to disappoint in a
real room**: the thresholds come from corpus recordings, and a noisy room, a
different microphone, or a family member with a similar voice could narrow the
margin they rely on. If your own voice gets gated, say so — that is the failure
that matters and the one this cannot be tuned for without your recordings.

`deepfilternet3` and `hush` are block stages with about eight seconds of
latency. They are for offline comparison; do not expect them to be usable in a
call until their graphs are re-exported with explicit recurrent state.

## 9. Use it in a real call

Select BlackHole as the microphone in Zoom, Meet, or Discord and have a
conversation. Watch for:

- Whether the far end reports the audio as clipped, thin, or gated.
- Whether meeting apps object to the device's reported latency
  (`docs/tech-research.md` §13, open question 6).

## 10. Long-session drift

The reason for the private aggregate device is that the microphone and the
virtual device run on different clocks; unhandled drift produces a click every
few minutes (`docs/tech-research.md` §4.2, open question 4). Leave it running
for **two hours or more** with the menu open occasionally, and check that the
dropout count stays at zero.

This is the single most important unverified claim in the whole design. Drift
compensation is configured on the virtual device with the microphone as the
clock source, but whether that alone stays glitch-free over a long meeting has
never been measured.

## Known unknowns

Written down because they are the places a failure is most likely, and because
none of them can be checked without hardware:

| Area | What might be wrong |
|---|---|
| Aggregate channel layout | The callback assumes the first input buffer belongs to the microphone, because the aggregate lists sub-devices in the order given and the microphone is given first. If a device presents several input streams, the wrong one may be read — the **In** meter would move but the audio would be from the wrong source |
| Buffer size negotiation | The HAL clamps the requested 256 frames to what the device supports. The scratch buffers are sized from what it actually granted, but a device that changes its mind later has not been handled |
| Drift compensation keys | The aggregate dictionary is built from the documented key strings. A wrong key is accepted silently and simply does nothing, so the only symptom would be drift reappearing over a long session |
| Device removal while running | Unplugging the microphone mid-session is not handled. Expect it to stop rather than recover |
| Aggregate cleanup after a crash | The aggregate is destroyed on drop. If the app is killed, one may linger; it is private so it will not appear in Sound Settings, but `Audio MIDI Setup` will show it |
| Audio workgroup | The inference thread is a plain thread. `docs/tech-research.md` §9 calls for joining the device's `os_workgroup` for correct scheduling, which has not been done and may show up as jitter under load |

## Reporting a problem

Collect, in this order: the log from step 2, the dropout count, the model in
use, and the output of `cargo run -p noican-cli -- list`. If the audio itself is
wrong rather than absent, capture the same input through the CLI (step 1) — that
separates a model problem from a plumbing problem, which is the first thing
worth knowing.
