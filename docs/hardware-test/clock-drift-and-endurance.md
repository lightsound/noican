# Hardware test: Clock drift and endurance

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Clock drift and endurance

The physical microphone and virtual output use different clocks; this is the
acceptance test for Aggregate Device drift compensation.

1. Use a physical USB microphone where possible, because its clock is
   clearly independent from BlackHole.
2. Run a continuous two-hour recording through Noican.
3. Speak or play a short reference tone every five minutes.
4. Inspect the entire file for discontinuities and measure reference-tone
   spacing.
5. Pass criteria:
   - no periodic click, duplicate block, or dropped block,
   - no increasing timing error,
   - no engine fault,
   - bounded memory use,
   - Aggregate Device remains alive.
6. Repeat sleep/wake, microphone disconnect/reconnect, and app quit/relaunch.
   Resource teardown must not leave a visible or reusable stale aggregate.

Record this result separately for each macOS major version under support,
especially macOS 26.

Scored on 2026-09-10
([record](../acceptance/2026-09-10-long-session-owner-report.md)) from
an owner report, not a measured run: a meeting of about two hours with a
Shure MV7i and a light 48 kHz denoiser, no reference tone, no recording kept,
and the microphone's rate — hence the transport — not read (the MV7i
took the aggregate path in every earlier record; that is an inference
here). Of the step 5 criteria, only "no engine fault" passes, on the
owner's observation that the microphone stayed live for the whole
meeting; "Aggregate Device remains alive" is not covered because the
path is unconfirmed, and the click/block, timing-error and memory
criteria, steps 3–4, and step 6 are not covered. A measured two-hour
run is still outstanding.
