# Result record — Developer ID build captures the microphone (audio-input entitlement), 2026-09-09

Hardware run for PR #33 (branch head `0ac310e`), which signs
`Noican.app` with `macos/Resources/Noican.entitlements`
(`com.apple.security.device.audio-input`) on both signing paths and
fails the build if the entitlement is absent from the finished
signature. The run checks the one thing the change exists for: a
**Developer ID, hardened-runtime** bundle must show the microphone
prompt and capture audio, where the pre-fix bundle was refused by
`tccd` without a prompt (Observations of
[2026-09-09-split-render-format.md](2026-09-09-split-render-format.md)).
It follows the Build section of
[docs/macos-hardware-test.md](../macos-hardware-test.md). Audio
measurements were not part of this run; see "Not covered".

## Environment

| Item | Value |
|---|---|
| Mac model / chip | MacBook Air — Apple M2 (the machine of the earlier 2026-09-09 record; assumed unchanged) |
| macOS / Xcode | macOS 26.6.2 (25G83) / Xcode 26.6 (assumed unchanged since the earlier record of the same day) |
| App commit | `0ac310e` (PR #33 head); `scripts/build-macos-app.sh`, Rust staticlib release, Swift release |
| App signature | **Developer ID** (`Developer ID Application: Qin, G.K. (6R926386F6)`), `flags=0x10000(runtime)`, timestamped; entitlement `com.apple.security.device.audio-input = true` |
| App process | PID 83336 (fresh launch after `tccutil reset`) |
| Driver | Noican.driver `0.1.0` (2 channels), Developer ID — unchanged from the earlier record |
| Microphones | Bluetooth headset (split transport); built-in microphone (aggregate path) |
| TCC state before the run | `tccutil reset Microphone com.lightsound.noican` (the ad-hoc build had already been granted on this Mac) |

## Measured (verbatim)

Build and signature (step 1):

```
Finished `release` profile [optimized] target(s) in 4.08s
Build complete! (4.95s)
/Users/ks/ghq/github.com/lightsound/noican/noican/dist/Noican.app: replacing existing signature
/Users/ks/ghq/github.com/lightsound/noican/noican/dist/Noican.app
Executable=/Users/ks/ghq/github.com/lightsound/noican/noican/dist/Noican.app/Contents/MacOS/NoicanMenuBar
[Dict]
	[Key] com.apple.security.device.audio-input
	[Value]
		[Bool] true
CodeDirectory v=20500 size=111777 flags=0x10000(runtime) hashes=3482+7 location=embedded
Executable Segment flags=0x1
Authority=Developer ID Application: Qin, G.K. (6R926386F6)
Authority=Developer ID Certification Authority
```

The script's own post-sign gate passed (the last line of the script is
the bundle path; a missing entitlement exits 1 before it).

`tccd` (`/usr/bin/log show --last 5m --predicate 'process == "tccd"' |
grep -i noican`, JST, excerpt; PID 83336 throughout):

```
18:01:29.635 Publishing <TCCDEvent: type=Delete, service=kTCCServiceMicrophone, identifier_type=Bundle ID, identifier=com.lightsound.noican>
18:01:30.687 Error  Prompting policy for hardened runtime; service: kTCCServiceAppleEvents requires entitlement com.apple.security.automation.apple-events but it is missing for accessing={TCCDProcess: identifier=com.lightsound.noican, pid=83336, ...}, requesting={TCCDProcess: identifier=com.apple.appleeventsd, pid=570, ...}
18:01:38.807 AUTHREQ_PROMPTING: msgID=27307.236, service=kTCCServiceMicrophone, subject=Sub:{com.lightsound.noican}Resp:{TCCDProcess: identifier=com.lightsound.noican, pid=83336, ...}
18:01:39.832 Publishing <TCCDEvent: type=Create, service=kTCCServiceMicrophone, identifier_type=Bundle ID, identifier=com.lightsound.noican>
```

No line containing `kTCCServiceMicrophone requires entitlement` appears
in the window (the full output was read; the remaining lines are
`AUTHREQ_ATTRIBUTION` / `AUTHREQ_SUBJECT` / `staticCode` bookkeeping
for WindowServer, coreaudiod, syspolicyd and tccd itself).

What the log establishes:

- `type=Delete` at 18:01:29 is the `tccutil reset`; the run started
  from no microphone record.
- `AUTHREQ_PROMPTING … kTCCServiceMicrophone` at 18:01:38 is `tccd`
  deciding to **prompt** the hardened-runtime process — the decision the
  pre-fix bundle never reached — and `type=Create` one second later is
  the operator's grant.
- The only `requires entitlement … but it is missing` line is for
  **`kTCCServiceAppleEvents`** at launch (+0.2 s after start,
  requested by `appleeventsd`), not for the microphone. See
  Observations.

## Results

| # | Check (Build section, docs/macos-hardware-test.md) | Result |
|---|---|---|
| 1 | Developer ID build succeeds; `codesign --display --entitlements -` lists `com.apple.security.device.audio-input` = `true`; `--verbose=4` shows `flags=0x10000(runtime)` and the Developer ID authority | **Pass** — verbatim above |
| 2 | After `tccutil reset Microphone com.lightsound.noican`, launching the Developer ID bundle and starting the engine shows the **microphone permission prompt** | **Pass** — operator saw the prompt; corroborated by `AUTHREQ_PROMPTING … kTCCServiceMicrophone` and the `type=Create` event |
| 3 | After granting, the meters follow the voice and Preview is audible, on a Bluetooth headset and on the built-in microphone | **Pass (operator report)** — reported "OK" for the step as worded; no recording or level measurement was taken |
| 4 | `tccd` log contains no `kTCCServiceMicrophone requires entitlement … but it is missing` | **Pass** — none in the 5-minute window that includes the reset, launch, prompt, and grant |

### Not covered

- **Audio measurements** (per-channel RMS/peak of a recording,
  level-integrity and native-rate criteria): not taken. This run
  establishes *that* the Developer ID bundle captures, not its level;
  the 2026-09-09 split-render-format record (same code paths, ad-hoc
  signature) remains the level evidence.
- **Ad-hoc regression**: the ad-hoc build now also carries the
  entitlement; it was built and its signature inspected on the
  development machine and in CI, but was not launched in this run.
  Nothing in the change affects TCC on the ad-hoc path (no hardened
  runtime), so this is a low-risk gap.
- **Gatekeeper / notarization**: the bundle was launched from the build
  directory with `open`; download quarantine and notarization are out
  of scope of PR #33.
- **A Mac with no prior Noican TCC record** (first install rather than
  a reset): the reset is the documented substitute.
- Model switching, endurance, drift — unchanged code, not exercised.

### Observations

- **`kTCCServiceAppleEvents requires entitlement
  com.apple.security.automation.apple-events but it is missing`** is
  logged once, 0.2 s after launch, with `appleeventsd` as the
  requester. The app sends no Apple Events itself — `rg` over
  `macos/Sources` finds no `NSAppleScript` / `NSAppleEventDescriptor` /
  `NSAppleEventManager` / `NSWorkspace` use; the only launch-time
  candidates are AppKit's own Launch Services handshake (`open` delivers
  the launch event) and `NSApp.activate()` in `StatusBarController`.
  Nothing user-visible failed (prompt shown, capture and Preview work,
  the popover opens), so the entitlement is **not added**: PR #33's rule
  is that every hardened-runtime key must be justified by something
  that breaks without it, and nothing does. It is recorded here so a
  future `tccd` grep is not mistaken for a microphone regression — the
  discriminating text is `service: kTCCServiceMicrophone`, and the
  hardware plan's grep now filters on `requires entitlement` so both
  lines are visible and told apart by service name.
- The `Prompting policy for hardened runtime` text is the same for both
  services; only the `service:` field differs. Read the service name,
  not the sentence.
