# Noican Virtual Audio Driver

The Noican driver is the Phase 1 "own the device" deliverable
(docs/tech-research.md §3, §12): a BlackHole fork built with the
joycast.driver pattern, one channel at 48 kHz, device name
`Noican Microphone`, UID `com.lightsound.noican.mic_UID`.

Its source lives in its own public GPL-3.0 repository,
[lightsound/noican-driver](https://github.com/lightsound/noican-driver),
which is the complete corresponding source for every driver Noican ships.
That repository owns the driver's build (`scripts/build-driver.sh`, the
pinned BlackHole submodule, the icon, the GPL notice) and documents its
identity, the injected macros, the CFPlugIn factory UUID, and the version
history in its `docs/driver.md`. This document covers only how this
repository consumes the driver and how the app recognizes it.

## How this repository consumes the driver

`external/noican-driver` is a git submodule of `lightsound/noican-driver`;
it carries BlackHole as a nested submodule (`external/blackhole` inside
it, pinned to the upstream release tag `v0.7.1`). The gitlink pins the
exact driver revision this repository builds and ships.

```bash
git submodule update --init --recursive           # once, after cloning

# Ad-hoc build (compile check; loadable only on SIP-relaxed dev machines):
bash external/noican-driver/scripts/build-driver.sh

# Developer ID build (the installable artifact; macOS 15+ coreaudiod
# only loads Developer-ID-signed drivers):
NOICAN_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
  bash external/noican-driver/scripts/build-driver.sh

# Install (copies to /Library/Audio/Plug-Ins/HAL, restarts coreaudiod):
bash external/noican-driver/scripts/install-driver.sh

# Uninstall (removes the bundle, restarts coreaudiod):
bash external/noican-driver/scripts/uninstall-driver.sh
```

The build writes `external/noican-driver/dist/Noican.driver` (ignored by
the driver repository). On macOS 26, `sudo launchctl kickstart -k
system/com.apple.audio.coreaudiod` is rejected by SIP; the scripts restart
the daemon with `killall coreaudiod` instead (docs/macos-hardware-test.md).

Changing the driver means a pull request to `lightsound/noican-driver`
first, then bumping the gitlink here:

```bash
git -C external/noican-driver fetch origin
git -C external/noican-driver checkout <commit-or-driver-v-tag>
git -C external/noican-driver submodule update --init
git add external/noican-driver
```

A released driver is tagged `driver-v<version>` in the driver repository
(`<version>` is the bundle's `CFBundleShortVersionString`), and the
bundle's `Contents/Resources/LICENSE` points recipients there; a release
of this repository must pin the gitlink to such a tag (docs/release.md).

CI (the macOS job) checks out the submodules recursively and runs the
ad-hoc build so a gitlink bump that breaks the driver build is caught;
loading, Audio MIDI Setup visibility, and loopback behavior can only be
verified on hardware with a Developer ID build (see the Driver check in
docs/macos-hardware-test.md).

## How the app recognizes the driver

Both app-side matchers lowercase the device UID and prefix-match
`com.lightsound.noican.` — **with the trailing dot** — so they accept the
current UID and the 0.1.0 driver's `com.lightsound.noican.2ch_UID`
without modification, and one app build runs with either driver:

- Swift: `AudioDeviceCatalog.isNoicanVirtualDevice`
  (macos/Sources/NoicanMenuBar/CoreAudioDevices.swift)
- Rust: `is_noican_loopback_uid`
  (crates/noican-coreaudio/src/monitor.rs; unit tests pin the exact UIDs
  of the current and the 0.1.0 driver, and the negative case
  `com.lightsound.noican_UID`)

The UID base is set in the driver repository, whose build refuses a base
outside that prefix; because that guard and the matchers now live in
different repositories, the macOS CI job here also checks that the built
driver binary carries a `com.lightsound.noican.<segment>_UID` string, so
a gitlink bump to a driver the app cannot recognize fails CI.

Tell an installed bundle's shape from its version string (0.1.0:
2 channels, 0.2.0: 1 channel):

```bash
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  /Library/Audio/Plug-Ins/HAL/Noican.driver/Contents/Info.plist
```

The widths differ (0.2.0 one channel; 0.1.0 and stock BlackHole 2ch two),
and the app does not care: both transports size their render format from
the virtual output they are given — one client channel per device
channel, the mono engine sample in each — so consumers get a mono
recording from the current Noican device and a dual-mono one from a
2-channel device. How a consumer records from the 1-channel device was
settled on hardware: an AVFoundation capture produced `1 ch, 48000 Hz`
files on all three microphone paths, and QuickTime kept the device
selected across the UID change
([2026-09-11 record](acceptance/2026-09-11-1ch-driver.md)).

## Coexistence with stock BlackHole

`AudioDeviceCatalog.isNoicanVirtualDevice` continues to match **both**
`BlackHole2ch_UID` and the `com.lightsound.noican.` prefix: both are
loopbacks Noican can feed, and both must stay excluded from the
microphone picker and refused as preview-monitor targets.

Selection priority is defined in `AudioDeviceCatalog.virtualOutput(in:)`:
**the Noican driver wins when both are installed**, and stock BlackHole
2ch remains the fallback when the Noican driver is absent (the Phase 0
setup). Rationale: the Noican driver is the device this project brands,
signs, and tests, while stock BlackHole may be shared with — and
reconfigured by — other software. Before this rule the pick fell to
Core Audio's device-enumeration order, which is undefined.

The Rust preview-monitor policy is intentionally broader: it refuses any
`virt`-transport device, any UID containing `BlackHole`, and the Noican
prefix, so the preview can never loop into any loopback regardless of
which one the engine feeds.

## Licensing boundary

- The driver is **GPL-3.0** (BlackHole's license); its notice, the
  source-availability statement, and the full GPL text are the driver
  repository's `LICENSE`, embedded in the bundle as
  `Contents/Resources/LICENSE`.
- The driver is a separate program loaded by `coreaudiod`. **Never add its
  sources or objects to the app targets** — the GPL must not extend to the
  app (docs/tech-research.md §11). Nothing under `external/` is part of
  the Cargo workspace or the Swift package.
