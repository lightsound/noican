# Phase 2 controls: settings persistence, launch at login, strength

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Settings persistence

The app persists the selected microphone UID, model id, and strength in
`UserDefaults` and restores them on the next launch through a reducer
event. The mode is deliberately never restored: the app always launches
Off, so starting it never captures the microphone (TCC prompts and live
capture must follow a user action). The login-item state is not
persisted by the app — `SMAppService` itself is the source of truth and
is re-read at launch.

1. Select a non-default microphone (e.g. a USB microphone), a
   non-default model (e.g. `DPDFNet2 48k HR`), and a non-default
   strength (e.g. 60%). Quit and relaunch the app.
2. The microphone checkmark, the Model picker, and the strength slider
   must show the persisted values; the mode must be **Off** and no
   microphone capture may start (no TCC prompt, no aggregate device in
   Audio MIDI Setup, no level movement).
3. Select On: the engine must start with exactly the restored
   microphone, model, and strength.
4. Unplug the USB microphone, relaunch, and confirm the selection falls
   back to another device while the app stays usable. Plug the USB
   microphone back in, relaunch again: the stored selection must return
   (a temporarily missing device must not overwrite the preference).
5. While running, switch to a model whose weights are not downloadable
   (e.g. disconnect the network and pick an unfetched model): after the
   failure reverts the picker, relaunch — the picker must restore the
   model that was actually running, not the failed pick.

## Launch at login

Registration uses `SMAppService.mainApp` and depends on the app's
location and signature; development builds run from `dist/` commonly
cannot register. CI can only compile this path — every claim below is
hardware-only. Test with the app copied to `/Applications` (Developer ID
build) for the positive path.

1. Toggle "Start at login" on in the menu footer. The toggle must
   reflect the real outcome: on success it stays on and the app appears
   under System Settings → General → Login Items; on failure it snaps
   back off with the reason shown under the toggle (no silent failure,
   no toggle stuck in a state `SMAppService` does not report).
2. Log out and back in (or reboot): the app must launch into the menu
   bar, mode Off, without capturing the microphone.
3. Toggle it off: the entry must disappear from Login Items, and after
   a re-login the app must not start.
4. *(Development build from `dist/`)* Toggle it on: if registration
   fails, the failure must be visible — toggle back off plus the error
   text — rather than pretending success.

## Strength control

The strength slider (0–100%, default 100%) blends the processed signal
(wet) with the raw microphone (dry) inside the inference worker,
upstream of the output ring and the preview monitor tee — Preview and
the virtual microphone always hear the same mix. The dry path is
delay-compensated by the active model's reported latency, so a partial
mix must not comb-filter or double the voice. The value crosses to the
engine as one atomic: moving the slider never rebuilds the transport.

1. Select On with a noisy source present (fan, typing) and record from
   the virtual device; watch the level meters.
2. At 100%: unchanged Phase 0/1 behavior (full suppression).
3. Drag toward 0% while speaking: background noise must fade back in
   smoothly — no clicks, no zipper steps, no dropouts, and the engine
   must not restart (status stays `Running`, meters keep moving, no
   busy spinner).
4. At 0%: the recording must be the (delay-aligned) raw microphone.
5. At 50%, speak continuously: the voice must sound like one voice, not
   a doubled/hollow "comb-filtered" voice. If any model still sounds
   comb-filtered at 50%, record which model — its reported
   `latency_samples()` (resampling + frame buffering + declared
   algorithmic delay) may underestimate its true end-to-end delay; the
   mismatch, not the mixer, is the bug to file.
6. Repeat 3–5 in Preview mode with headphones: the audible mix must
   track the slider exactly like the recording does.
7. Switch models while at 50%: only the bounded switch fade, no click
   from the dry-compensation delay jumping between the models'
   different latencies (the jump lands inside the silent fade
   boundary).
8. Relaunch: the slider must restore its persisted position.

## Acceptance checklist (Phase 2 controls)

Run the Settings persistence, Launch at login, and Strength control
procedures above; the build passes when:

1. **Selections survive a relaunch**: microphone, model, and strength
   restore from `UserDefaults`; a temporarily unplugged microphone does
   not lose its stored preference.
2. **Always launches Off**: no mode restoration, no TCC prompt, no
   capture, and no aggregate device at launch — in every persistence
   scenario, including launch-at-login starts.
3. **Login item registers honestly**: the toggle shows the re-read
   `SMAppService` status; a failed registration snaps the toggle back
   with the reason under it (never a silent failure or an optimistic
   toggle).
4. **Login item round-trips**: on (from `/Applications`) → appears in
   Login Items and auto-starts after re-login; off → disappears and
   stays stopped.
5. **Strength blends without artifacts**: smooth dry/wet transition
   across the whole slider range while running — no clicks, zipper
   noise, restarts, or busy states.
6. **No comb-filter at partial strength**: at 50% the voice stays
   single (dry path delay-compensated) on every listed model.
7. **Preview parity**: the preview monitor plays the same mix the
   virtual microphone receives at every slider position.
8. **Strength × switching**: model switches at partial strength produce
   only the bounded fade, with no click from the latency change.

### Acceptance record (Phase 2 controls, 2026-08-27, owner-run)

Run against the PR #14 build (`ad7defd`) on Apple hardware:

- **Settings persistence (A1–A5): pass** — all five checks, including the
  unplugged-microphone preference survival and the failed-switch revert.
- **Launch at login (B1–B4): functional pass** — registration, re-login
  round-trip, and failure surfacing all behaved. Design follow-up: the
  switch-style toggle was visually overweight for a footer setting and
  was restyled to a checkbox afterwards.
- **Strength control: pass except C5** — smooth blending, preview
  parity, persistence, and switching all passed. **C5 failed on DPDFNet2
  and DPDFNet8** (doubled voice at 50%); every other model passed. Root
  cause per the procedure's own diagnosis: both DPDFNet profiles carry
  4 hops (1920 samples, 40 ms) of architectural lookahead their ONNX
  metadata does not report, so `output_delay()` under-reported and the
  dry path ran 40 ms early. Fixed by a measurement-backed lookahead
  constant (cross-correlation of real speech against the aligned CLI
  output now measures 0 residual lag on both profiles). **C5 re-verified
  on hardware 2026-08-28 (owner-run, PR #15 build): the doubled voice at
  50% strength is resolved on both DPDFNet2 and DPDFNet8.**
- Observation, not a defect: transient noises (trackpad/keyboard clicks)
  pass through some models at 100% strength — click suppression is a
  property of each model (DPDFNet8, DeepFilterNet3, and Hush removed
  clicks; the lighter models let them through). Lowering the strength
  mixes raw-microphone clicks back in on every model, by design.
