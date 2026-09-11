# Hardware test: Preview (self-monitor) and level meters

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Preview (self-monitor)

Preview mode runs the engine and additionally plays the processed
microphone signal on the system default output device through a second,
output-only AUHAL fed by a dedicated monitor ring. It shares no state
with the meeting-facing path except the lock-free tee in the inference
worker. Preview and On both feed the virtual microphone; switching
between them only arms or disarms the monitor.

1. Connect wired headphones and make them the system default output.
2. Select Preview in the menu (directly from Off, or from On).
3. Speak: the processed voice must be audible with a modest constant
   delay (engine latency plus ~40 ms of monitor ring priming). The delay
   is by design, not a defect. The status line reads
   `Previewing`.
4. Headphones are mandatory: through speakers the processed microphone
   feeds back into itself (Phase 0/1 has no AEC). There is deliberately
   no persistent warning text — unsafe outputs are refused on press with
   the reason shown, and the feedback guard explains itself when it
   trips.
5. Switch models while previewing: the voice must keep playing across
   the switch with only the bounded fade — no click, no full-scale
   burst, no dropout beyond the fade.
6. Switch Preview → On: playback must stop immediately; the engine keeps
   running without interruption.
7. While recording from the virtual device in QuickTime, switch between
   Preview and On: the recording must be unaffected.
8. Set the system default output to each of the following and press
   Preview:
   - the BlackHole/Noican loopback (the preview would reach the meeting
     twice),
   - a Multi-Output Device (Audio MIDI Setup) containing BlackHole (the
     aggregate can hide the meeting loopback, and the feedback guard
     cannot catch that route),
   - the built-in speakers (the voice would feed straight back into the
     microphone).
   The press must be refused in place: the mode and the engine (whether
   Off or On) stay exactly as they were — Off never starts the engine —
   and one short line ("Preview needs headphones — <cause>.") explains
   the reason under the control, without device UIDs. With the message
   showing, switch the default output back to headphones: the message
   must clear within about a second, and pressing Preview must then
   work.
9. A monitor failure at runtime (one that passed the pre-flight check),
   including a feedback-guard trip: the pill stays on Preview with a red
   warning tint, the engine keeps running (status returns to
   `Running`), and the reason renders under the control.
   Re-tapping Preview retries the monitor.
10. Select Off, then Preview again: the preview must come back cleanly
    with no stale audio replayed and no double playback.
11. Compare % CPU in Activity Monitor between On and Preview: the
    increase must be small (the monitor path only copies samples).
12. The monitor clock is not drift-corrected: over long previews an
    occasional short gap (underrun re-prime) or discarded block
    (overrun) is acceptable; persistent crackle is not.
13. *(Optional, external speakers required)* With USB/Bluetooth speakers
    as the default output — which the device-type check cannot classify —
    select Preview and raise the volume until feedback starts: within
    about half a second of sustained near-clipping output the feedback
    guard must silence the preview on its own; the pill stays on Preview
    with the red warning tint, the engine keeps running, and the menu
    explains why. Re-tapping Preview re-arms the guard and the monitor.
14. **Headphone jack unplug** *(wired headphones in the built-in jack)*:
    with the jack as the default output, select Preview, then unplug the
    headphones while it plays. The preview must stop itself within about
    a second — before anything audible comes out of the internal
    speakers beyond a moment of bleed — with the pill staying on Preview
    in the red warning tint, the engine still running (status returns to
    `Running`), and "Preview stopped: …" under the control. Two
    machine-dependent paths must both land here: on most Macs the same
    built-in device flips its data source from `'hdpn'` to `'ispk'`
    (caught by a data-source listener on the monitor's own device, plus
    the 1 Hz health poll as backstop); on machines where the jack is a
    separate device, the device disappears (caught by the device-list
    path, with a "device was disconnected" reason instead). Plug the
    headphones back in and re-tap Preview: the monitor must come back on
    the vetted output.

Changing the default output while previewing does not retarget the
monitor in this version; switch to On and back to Preview to pick up the
new device. The *safety* of the device the monitor actually plays on is,
however, watched continuously while the preview plays (step 14): the
enable-time-only vetting was a known hole where unplugging the jack let
the refused internal speakers keep playing with only the feedback guard
as insurance. Note the watcher re-vets the monitor's own device via
`noican_monitor_device_error`, not `noican_monitor_target_error` — the
latter judges the *current default output*, which may already have moved
elsewhere while the monitor stays on the old device.

## Level meters

The inference worker publishes per-block (10 ms) input (pre-model) and
output (post-model) peak levels with a short exponential decay; the menu
polls them at ~20 Hz only while the popover is open. The meters draw on
a shared −60…0 dB scale and are shown only while the engine runs.

1. Select On (or Preview) and open the popover.
2. Speak: the input bar must move with your voice, and the output bar
   must follow while you speak.
3. Stay silent with steady noise present (fan, keyboard typing): the
   output bar must sit clearly below the input bar — visual confirmation
   that noise suppression is working without listening to the stream.
4. Switch models while watching: the bars must not spike to full scale,
   freeze, or oscillate wildly (only the bounded switch fade).
5. The meters must move identically in Preview and On.
6. Select Off: the monitoring section disappears (and reappears at zero
   on the next start).
7. Close the popover and watch the app in Activity Monitor for a minute:
   CPU use and wake-ups must drop back to idle. The menu content (and
   with it the 20 Hz level poll) is built when the status-item popover
   opens and torn down when it closes — this check catches a lifecycle
   regression on the tested OS version.

## Acceptance checklist (preview + level meters)

Run the Preview and Level meters procedures above; the build passes when:

1. **Mode control**: Off / Preview / On transitions all work in both
   directions; Preview ↔ On switches are instant and never interrupt the
   virtual-microphone path.
2. **Preview audible**: in Preview mode, your own processed voice is
   heard on headphones with a small constant delay; switching to On
   stops it immediately.
3. **Switching under preview**: model switches while previewing produce
   no dropout beyond the bounded fade and no full-scale burst.
4. **Main path isolation**: a QuickTime recording from the virtual
   device is unaffected by Preview ↔ On switches.
5. **Unsafe output refusal**: pressing Preview while the default output
   is the BlackHole/Noican loopback, a Multi-Output/aggregate device, or
   the built-in speakers is refused in place — mode and engine
   unchanged, the reason shown under the control — and the message
   clears within about a second of a safe output returning.
6. **Feedback guard** *(optional; needs external speakers)*: sustained
   feedback through an unclassifiable output stops the preview by itself
   within ~1 s; the pill stays on Preview with the red warning tint and
   the menu explains why.
6b. **Jack-unplug auto-stop**: unplugging wired headphones from the
    built-in jack while previewing stops the preview by itself within
    about a second with "Preview stopped: …" under the control; the pill
    stays on Preview with the red warning tint and the engine keeps
    running. On machines whose jack is a separate device, the
    device-loss path must produce the same result with a
    "device was disconnected" reason.
7. **Restart coherence**: Off → Preview brings the preview back cleanly,
   with no stale audio and no double playback.
8. **Intent is never moved**: on any failure (start failure, device
   loss, monitor failure, feedback trip) the pill keeps the user's
   selection with a red warning tint and the reason below the control;
   re-tapping the segment retries, and selecting a working microphone
   restarts into the selected mode automatically.
9. **Preview cost**: % CPU does not increase materially in Preview mode.
10. **Input meter follows speech**: the input bar moves when you speak.
11. **Suppression visible**: during noise-only passages (fan, typing)
    the output bar sits clearly below the input bar.
12. **Meter stability**: meters do not spike or freeze across model
    switches; the monitoring section is hidden while Off (and while a
    failure is shown) and returns at zero on the next start.
13. **Microphone switching**: changing the microphone while running
    rebuilds the transport with the same model and mode after a brief
    gap; hot-plugged devices appear in the list automatically.
14. **Mode-control animation**: the sliding pill stays visually intact
    while multi-line status/error text appears and disappears around it.
15. **No transitional flash**: pressing a pill while a failure is shown
    never flashes optimistic UI — no momentary blue pill, no meters
    sliding in and out, no error text blinking away and back. The view
    changes once, when the attempt settles (sections, colors, and error
    text render from the last settled state; the spinner and the status
    line are the only transitional feedback).
