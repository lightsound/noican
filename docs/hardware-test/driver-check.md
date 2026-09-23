# Hardware test: Driver check

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Driver check

The subject is the Noican driver (docs/driver.md). Stock BlackHole 2ch
remains acceptable as the Phase 0 fallback, but the acceptance record for
Phase 1 must cover the Noican driver.

1. Build the driver with a Developer ID identity and record the exact
   command and output:

   ```bash
   NOICAN_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
     bash external/noican-driver/scripts/build-driver.sh
   ```

2. Install it (copies to `/Library/Audio/Plug-Ins/HAL/Noican.driver` and
   restarts the audio daemon):

   ```bash
   bash external/noican-driver/scripts/install-driver.sh
   ```

   If restarting manually: on macOS 26, SIP rejects
   `sudo launchctl kickstart -k system/com.apple.audio.coreaudiod`; use
   `sudo killall coreaudiod` instead.
3. Open Audio MIDI Setup and confirm **Noican Microphone** appears as a
   **one-channel** 48 kHz device (manufacturer `lightsound`). A
   two-channel device means the 0.1.0 driver is still loaded — check the
   installed bundle's version (docs/driver.md) and that
   `coreaudiod` was restarted. Quit and relaunch the app after the
   driver swap and confirm the PID changed (see "Build" in
   [setup.md](setup.md)).
4. Confirm the device has both output and input streams and can loop a test
   signal before involving Noican.
5. Record the installed bundle signature and version:

   ```bash
   codesign --verify --deep --strict --verbose=2 \
     "/Library/Audio/Plug-Ins/HAL/Noican.driver"
   /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
     /Library/Audio/Plug-Ins/HAL/Noican.driver/Contents/Info.plist
   ```

6. Coexistence: with stock BlackHole 2ch also installed, start the engine
   and confirm the private aggregate is composed around the **Noican**
   device (`com.lightsound.noican.mic_UID`; the 0.1.0 driver's
   `com.lightsound.noican.2ch_UID` is also a Noican device to the app),
   not `BlackHole2ch_UID` — the app prefers the Noican driver when both
   are present (docs/driver.md, "Coexistence"). Then uninstall stock
   BlackHole (or test the Noican-only state first) and confirm the app
   still selects the Noican device.
7. Driver swap: the UID changed with the 1-channel driver, and meeting
   applications remember the microphone by UID. Open a meeting
   application (or QuickTime) that had Noican Microphone selected under
   the 0.1.0 driver and record whether it had to be selected again.
   Then confirm the reverse direction keeps working: uninstall, install
   the 0.1.0 (2-channel) build — or use stock BlackHole 2ch — and start
   the app: it must run, with dual-mono recordings as before. Return to
   the current driver afterwards.
8. Uninstall check (after the functional tests):
   `bash external/noican-driver/scripts/uninstall-driver.sh`, then confirm
   no Noican device remains in Audio MIDI Setup and
   `/Library/Audio/Plug-Ins/HAL/Noican.driver` is gone.

Do not disable SIP or use an ad-hoc driver signature for the acceptance test.

## Acceptance checklist (1-channel driver)

Run the Driver check (including step 7, the driver swap) with the 0.2.0 driver
installed, then the [composite](composite-microphone.md) and
[level-integrity](level-integrity.md) procedures; the build passes when:

1. **Shape**: Audio MIDI Setup lists Noican Microphone as a one-channel
   48 kHz device, `CFBundleShortVersionString` reads `0.2.0`, and the
   engine's `Aggregate composed` line reports the virtual output with
   `out 1`.
2. **Aggregate maps**: `Aggregate output routing` reads `channel map
   requested [0], … read back after initialize [0]` on the built-in
   microphone and `[-1, -1, 0]` / `[-1, -1, 0]` on the MV7i.
3. **Mono recordings, level unchanged**: QuickTime recordings from the
   Noican virtual microphone read `channels: 1` (or two identical
   channels, recorded as a QuickTime behaviour) on the built-in
   microphone, the MV7i, and a Bluetooth headset (split transport, whose
   `Split output routing` line reads `1` three times), and each RMS is
   within ±1.5 dB of the 2-channel driver's channel 0 for the same
   microphone (same-session swap preferred; otherwise the 2026-09-05
   figures: −19.6, −26.7, −21.0 dBFS).
4. **Preview and level notice**: Preview plays as before; the Noican
   Microphone slider at 50% shows the notice within a second and clears
   at maximum — the device's single control exists at one channel too.
5. **Re-selection recorded**: whether a meeting application (or
   QuickTime) that had Noican Microphone selected under the 0.1.0 driver
   needed it selected again after the UID change, as observed.
6. **Old driver still served**: with the 0.1.0 (2-channel) driver — or
   stock BlackHole 2ch — installed instead, the same app build starts and
   records dual mono as before; with stock BlackHole and the Noican
   driver both present the Noican device is preferred as before.

Scored on 2026-09-11
([record](../acceptance/2026-09-11-1ch-driver.md)): 1–4 and 6 pass, 5
recorded as "not needed" for QuickTime; stock BlackHole coexistence,
Driver check steps 4, 6 and 8 not covered. The level clause of
criterion 3 was evidenced by recording the Noican virtual microphone
and the raw physical microphone at the same moment under Passthrough
(difference ≤ 0.1 dB on every path and on both drivers), which removes
the operator's speech level from the comparison; that paired method is
acceptable evidence for the clause in later runs too.
