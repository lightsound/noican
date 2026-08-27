# Noican Virtual Audio Driver

The Noican driver is the Phase 1 "own the device" deliverable
(docs/tech-research.md §3, §12): a BlackHole fork built with the
joycast.driver pattern. Upstream
[ExistentialAudio/BlackHole](https://github.com/ExistentialAudio/BlackHole)
is a git submodule at `external/blackhole`, pinned to the upstream release
tag `v0.7.1`, and is **never patched** — every customization is injected at
build time through GCC preprocessor definitions and xcodebuild settings by
`scripts/build-driver.sh`. Updating upstream reduces to bumping the
submodule to a newer release tag and re-checking the macro list below
against the new `BlackHole/BlackHole.c`.

## Identity

| Item | Value | Rationale |
|---|---|---|
| Bundle | `Noican.driver` | Matches the app artifact naming (`Noican.app`) |
| Bundle ID | `com.lightsound.noican.driver` | Under the app's `com.lightsound.noican`; must equal `kPlugIn_BundleID` (upstream requirement) |
| Device name | `Noican Microphone` | The string meeting apps show in their microphone list; "Microphone" says what to pick it as |
| Device UID | `com.lightsound.noican.2ch_UID` | Matches both existing matchers verbatim (see below); `2ch` keeps room for future channel variants; `_UID` suffix keeps BlackHole's convention |
| Manufacturer | `lightsound` | Shown by Audio MIDI Setup / `system_profiler` |
| Channels | 2 | The engine's stereo loopback (Phase 0 contract) |
| Sample rates | 44.1 / 48 kHz | The app pins 48 kHz at start (`ensure48k`); 44.1 kHz stays available so other apps that insist on it do not error out |
| Version | `NOICAN_DRIVER_VERSION` (default `0.1.0`) | `MARKETING_VERSION` → `CFBundleShortVersionString` |

### How the UID is produced

BlackHole (v0.7.1) has **no `#ifndef` guard on its UID macros** — they are
derived, so the UID is controlled indirectly:

```c
// external/blackhole/BlackHole/BlackHole.c (kHas_Driver_Name_Format=false)
#define kBox_UID          kDriver_Name "_UID"
#define kDevice_UID       kDriver_Name "_UID"
#define kDevice2_UID      kDriver_Name "_2_UID"
#define kDevice_ModelUID  kDriver_Name "_ModelUID"
```

The build sets `kHas_Driver_Name_Format=false` (the default `true` splices
a `%ich` channel-count format into every UID) and
`kDriver_Name="com.lightsound.noican.2ch"`. `kDriver_Name` is not a
user-visible string in this configuration — the visible names are
`kDevice_Name` and `kManufacturer_Name` — so it can carry the reverse-DNS
UID base. Resulting identifiers:

- device UID: `com.lightsound.noican.2ch_UID`
- hidden mirror UID: `com.lightsound.noican.2ch_2_UID`
- model UID: `com.lightsound.noican.2ch_ModelUID`
- box UID: `com.lightsound.noican.2ch_UID`

Both app-side matchers accept these without modification, because each one
lowercases and prefix-matches `com.lightsound.noican.`:

- Swift: `AudioDeviceCatalog.isNoicanVirtualDevice`
  (macos/Sources/NoicanMenuBar/CoreAudioDevices.swift)
- Rust: `is_noican_loopback_uid`
  (crates/noican-coreaudio/src/monitor.rs; unit tests pin the exact UIDs)

### Full macro list injected by scripts/build-driver.sh

| Macro | Value | Guarded upstream? |
|---|---|---|
| `kDriver_Name` | `"com.lightsound.noican.2ch"` | `#ifndef` |
| `kHas_Driver_Name_Format` | `false` | `#ifndef` |
| `kDevice_Name` | `"Noican Microphone"` | `#ifndef` |
| `kDevice2_Name` | `"Noican Microphone Mirror"` | `#ifndef` |
| `kPlugIn_BundleID` | `"com.lightsound.noican.driver"` | `#ifndef` |
| `kPlugIn_Icon` | `"Noican.icns"` | `#ifndef` |
| `kManufacturer_Name` | `"lightsound"` | `#ifndef` |
| `kNumber_Of_Channels` | `2` | `#ifndef` |
| `kSampleRates` | `44100,48000` | `#ifndef` |
| `kDevice_IsHidden` / `kDevice_HasInput` / `kDevice_HasOutput` | `false` / `true` / `true` | `#ifndef` |
| `kDevice2_IsHidden` / `kDevice2_HasInput` / `kDevice2_HasOutput` | `true` / `true` / `true` | `#ifndef` |

Deliberately **not** passed:

- `kLatency_Frame_Size` — v0.7.1 defines it without an `#ifndef` guard;
  passing it would redefine the macro. The upstream default (0 frames) is
  what we want anyway.
- `kEnableVolumeControl`, `kCanBeDefaultDevice`,
  `kCanBeDefaultSystemDevice`, `kBox_Aquired` — upstream defaults kept.

xcodebuild settings (not preprocessor): `PRODUCT_NAME=Noican`,
`PRODUCT_BUNDLE_IDENTIFIER`, `MARKETING_VERSION`,
`ARCHS="arm64 x86_64"` (universal — the driver loads into `coreaudiod`,
whose architecture is the machine's, independent of the arm64-only app).

### CFPlugIn factory UUID

CFPlugIn factory UUIDs must be unique per plug-in, but upstream hardcodes
`e395c745-4eea-4d94-bb92-46224221047c` in `BlackHole.plist`, so every
unpatched BlackHole fork (stock BlackHole, JoyCast, ...) ships the same
one; when several coexist under `/Library/Audio/Plug-Ins/HAL`,
`coreaudiod`'s bundle scan logs duplicate-UUID warnings (harmless — each
driver runs in its own remote process — but noisy and confusing, observed
during hardware acceptance with JoyCast installed). The build rewrites the
built bundle's `Info.plist` with PlistBuddy to the Noican-unique factory
UUID `16ccdad9-e4f7-4cd4-8e81-520694b78514` (generated once for this
project; the HAL plug-in *type* UUID `443ABAB8-…` is Apple's and must not
change, and the factory function name stays `BlackHole_Create`). This is
bundle-metadata editing at build time, consistent with the no-source-patch
rule. The rewrite deletes the upstream key first, so an upstream bump that
changes the plist fails the build loudly instead of shipping a stale edit.

The primary device is the visible 2-in/2-out loopback and the mirror
device stays hidden — the same shape as stock BlackHole 2ch, which the
Phase 0 transport already passed hardware acceptance against.

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

## Build, install, uninstall

```bash
# Ad-hoc build (compile check; loadable only on SIP-relaxed dev machines):
bash scripts/build-driver.sh

# Developer ID build (the installable artifact; macOS 15+ coreaudiod
# only loads Developer-ID-signed drivers):
NOICAN_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
  bash scripts/build-driver.sh

# Install (copies to /Library/Audio/Plug-Ins/HAL, restarts coreaudiod):
bash scripts/install-driver.sh

# Uninstall (removes the bundle, restarts coreaudiod):
bash scripts/uninstall-driver.sh
```

On macOS 26, `sudo launchctl kickstart -k system/com.apple.audio.coreaudiod`
is rejected by SIP; the scripts restart the daemon with `killall
coreaudiod` instead (docs/macos-hardware-test.md).

CI (the macOS job) checks out the submodule and runs the ad-hoc build so
driver build breakage is caught; loading, Audio MIDI Setup visibility, and
loopback behavior can only be verified on hardware with a Developer ID
build (see the Driver check in docs/macos-hardware-test.md).

Expected benign compiler output (upstream code, unpatched by design):
`-Wformat-extra-args` on the `RETURN_FORMATTED_STRING` helpers — with
`kHas_Driver_Name_Format=false` the `CFStringCreateWithFormat` branch is
dead but still compiled, and our verbatim strings carry no `%i` — and the
upstream `MACOSX_DEPLOYMENT_TARGET = 10.10` deprecation warning. The
repo's warnings-as-errors gates cover the Rust and Swift app code, not
this GPL build of upstream C.

The device icon ships at `macos/Resources/Noican.icns` and is copied into
the bundle by the build (`kPlugIn_Icon`, served via
`kAudioDevicePropertyIcon`). The artwork is the Noican mascot — a singing
Japanese bush warbler (uguisu, a bird famed for its beautiful voice) at a
vintage microphone — generated for this project (AI-assisted, no
third-party rights). The icns carries all standard macOS icon types
(16–512 px plus @2x retina variants, 1024 px master embedded as
`ic10`); regenerate by re-masking a 1024 px square master with a
~236 px-radius rounded rectangle and composing with `icnsutil`.

## Licensing and trademarks

- The driver is **GPL-3.0** (BlackHole's license). `LICENSE.driver` at the
  repo root carries the notice, the source-availability statement, and the
  full GPL text; the build embeds it into the bundle as
  `Contents/Resources/LICENSE`.
- The complete corresponding source is this repository
  (`scripts/build-driver.sh` + the pinned `external/blackhole` submodule).
- The driver is a separate program loaded by `coreaudiod`. **Never add its
  sources or objects to the app targets** — the GPL must not extend to the
  app (docs/tech-research.md §11).
- The BlackHole name, logo, and branding are Existential Audio trademarks.
  The build strips `BlackHole.icns` and ships no BlackHole-branded
  user-facing strings. Known non-user-facing residues (accepted because
  source patches are off the table): the plug-in factory symbol
  `BlackHole_Create` referenced by the bundle's `Info.plist`, internal
  debug-log strings, and the default HAL box name (`"BlackHole Box"`, a
  hardcoded fallback inside the binary; the box is not shown in device
  lists).

## Uninstall residue

Removing the bundle removes the device. `coreaudiod` may keep the driver's
HAL *box* settings (name / acquired flag) in its own settings store; these
are a few bytes, inert without the driver, owned by macOS, and not safely
removable by an uninstaller — the acceptance criterion is that no Noican
device remains visible in Audio MIDI Setup after uninstalling.
