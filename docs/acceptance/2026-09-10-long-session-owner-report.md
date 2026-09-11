# Result record — long session, owner report, 2026-09-10

Owner report of a meeting of about two hours held through Noican with a
Shure MV7i and `FastEnhancer-B 48k`. It is the first entry against the
"Clock drift and endurance" section of
[docs/macos-hardware-test.md](../macos-hardware-test.md)
([hardware-test/clock-drift-and-endurance.md](../hardware-test/clock-drift-and-endurance.md)),
and it is an **observation record, not a scored run**: the procedure's
measurements were not made. No reference tone was played, no recording
was kept or inspected, and the sleep/wake, disconnect/reconnect and
quit/relaunch repetitions of step 6 were not performed. What the report
establishes is that a session of roughly the checklist's duration, on
the aggregate path, ran to the end without the other participants
remarking on anything — and the criteria below are scored no further
than that evidence carries.

## Environment

| Item | Value |
|---|---|
| Mac model / chip | Not recorded (the owner's Mac; the neighbouring records were made on a MacBook Air, Apple M2, but this run did not confirm it) |
| macOS / Xcode | Not recorded |
| App commit | Not recorded — the build installed on the owner's Mac on 2026-09-10, i.e. between PR #33 (`8635283`, merged 2026-09-09) and the 2026-09-11 driver run |
| App signature | Not recorded |
| Driver | Not recorded. Inference, not observation: the [2026-09-11 record](2026-09-11-1ch-driver.md) found `Noican.driver` 0.1.0 (2 channels) installed before its run, so 0.1.0 is the likely driver of this session |
| Microphone | Shure MV7i (composite USB microphone; at 48 kHz it takes the aggregate path, as in the 2026-09-05 and 2026-09-11 records — the rate was not re-read for this session) |
| Model / strength | `FastEnhancer-B 48k`; strength not recorded |
| Duration | About 2 hours, one continuous meeting |
| Recording | None kept |
| Measurement | None — no reference tone, no file inspection, no memory reading, no check of Audio MIDI Setup afterwards |

## Reported (owner, paraphrased)

The meeting lasted about two hours with the MV7i and FastEnhancer-B. No
reference tone was played and no recording was analysed; the other
participants did not point anything out, so the owner considers the
session free of problems.

## Results

Clock drift and endurance, step 5 pass criteria:

| Step | Criterion | Result |
|---|---|---|
| 5, bullet 1 | No periodic click, duplicate block, or dropped block | **Not covered** — no file was inspected (step 4). The weaker observation available: nobody on the call remarked on clicks or dropouts over about two hours |
| 5, bullet 2 | No increasing timing error | **Not covered** — no reference tone (step 3), no spacing measured (step 4) |
| 5, bullet 3 | No engine fault | **Pass (owner observation)** — the virtual microphone carried the owner's voice for the whole meeting; an engine fault silences it, which the participants would have reported. The popover was not inspected for a fault line |
| 5, bullet 4 | Bounded memory use | **Not covered** — not observed |
| 5, bullet 5 | Aggregate Device remains alive | **Pass (owner observation)** — same evidence as the previous row: audio kept flowing through the private aggregate for about two hours. Audio MIDI Setup was not checked for a stale aggregate afterwards |

Other steps:

| Step | Result |
|---|---|
| 1 Physical USB microphone | **Met** — Shure MV7i |
| 2 Continuous two-hour recording | **Partly** — a two-hour session, but not recorded |
| 3 Reference tone every five minutes | **Not covered** |
| 4 Whole-file inspection and tone spacing | **Not covered** |
| 6 Sleep/wake, disconnect/reconnect, quit/relaunch; no stale aggregate | **Not covered** |
| Per-macOS-version record | **Not covered** — the macOS version was not recorded |

## Not covered

- Everything measurement-based in the section: reference-tone spacing,
  discontinuity inspection, memory, and the post-session aggregate
  check. The first measured long session is still outstanding; when it
  is run, this record is superseded by it, not amended.
- Step 6 (sleep/wake, disconnect/reconnect, quit/relaunch).
- The split (native-rate) transport: the MV7i runs at 48 kHz on the
  aggregate path, so this says nothing about the split transport's own
  drift servo (native-rate checklist criterion 3 covers that, at 30
  minutes).
- Environment details (Mac, macOS, app commit, driver version) — see
  the table; nothing here should be read as tying the observation to a
  particular build.

## Observations

- The owner's evidence is the absence of complaints from the meeting's
  other participants. That is sensitive to a dead microphone and to
  gross, sustained artefacts, and insensitive to single dropped blocks,
  a slow timing drift, and memory growth — which is why the click/block, timing-error and memory
  criteria stay Not covered rather than passing on the same observation.
- The report answers §13 Open Question 4 of
  [docs/tech-research.md](../tech-research.md) ("Long-session (2 h+)
  stability of aggregate-device drift compensation") only to the extent
  above; the question stays open until a measured run exists.
