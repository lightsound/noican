#!/usr/bin/env bash
# Builds the Noican virtual audio driver (dist/Noican.driver) from the
# BlackHole submodule, following the joycast.driver pattern: the upstream
# source is never patched; every customization is injected at build time
# through GCC preprocessor definitions and xcodebuild settings, so upstream
# BlackHole updates reduce to bumping the submodule.
#
# Identity decisions (docs/driver.md records the rationale):
#   bundle          Noican.driver
#   bundle ID       com.lightsound.noican.driver
#   device name     "Noican Microphone"  (what meeting apps list)
#   device UID      "com.lightsound.noican.mic_UID"
#   channels        1, sample rates 44.1/48 kHz (Noican pins 48 kHz)
#   version         0.2.0 (0.1.0 was the 2-channel "com.lightsound.noican.2ch_UID"
#                   device; the version is how an installed bundle tells
#                   which shape it has — docs/driver.md, "History")
#
# The device UID cannot be set directly: BlackHole derives kDevice_UID as
# kDriver_Name "_UID" (with kHas_Driver_Name_Format=false), so kDriver_Name
# carries the reverse-DNS UID base. kDriver_Name is not user-visible; the
# visible strings are kDevice_Name and kManufacturer_Name. The UID base
# deliberately carries no channel count (the 0.1.0 "2ch" base did, and the
# name became false the moment the width changed); it must keep the
# trailing-dot prefix "com.lightsound.noican." that both app-side matchers
# test — Swift AudioDeviceCatalog.isNoicanVirtualDevice and Rust
# is_noican_loopback_uid (case-insensitive) — so a base of plain
# "com.lightsound.noican" (no dot) would produce "com.lightsound.noican_UID"
# and fail both. The app recognizes the 0.1.0 UID through the same prefix,
# so old and new drivers can be swapped under one app build.
#
# Signing follows scripts/build-macos-app.sh: Developer ID when
# NOICAN_CODESIGN_IDENTITY is set, ad-hoc otherwise. macOS 15+ coreaudiod
# only loads Developer-ID-signed drivers (ad-hoc needs a SIP-relaxed dev
# machine), so ad-hoc builds are compile checks, not installable artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BLACKHOLE="$ROOT/external/blackhole"

DRIVER_NAME="Noican"
BUNDLE_ID="com.lightsound.noican.driver"
# kDriver_Name: the UID base ("<base>_UID"), not a user-visible string.
# Keep the "com.lightsound.noican." prefix (see the header comment).
DEVICE_UID_BASE="com.lightsound.noican.mic"
DEVICE_NAME="Noican Microphone"
DEVICE2_NAME="Noican Microphone Mirror"
MANUFACTURER_NAME="lightsound"
ICON="Noican.icns"
# One channel: the engine signal is mono, and the app's transports render
# one client channel per virtual-output channel whatever the width (dual
# mono on a 2-channel device, plain mono here), so the device carries the
# shape of the signal — like the Krisp and JoyCast virtual microphones —
# with half the ring buffer of the 2-channel build. Consumers record mono.
CHANNELS=1
SAMPLE_RATES="44100,48000"
# Bumped with every change of device shape or UID so an installed bundle's
# Info.plist (CFBundleShortVersionString) says which one it is.
DRIVER_VERSION="${NOICAN_DRIVER_VERSION:-0.2.0}"
# CFPlugIn factory UUID: must be unique per plug-in, but upstream hardcodes
# one in BlackHole.plist, shared by every unpatched fork (stock BlackHole,
# JoyCast, ...); coexisting forks then trigger harmless but noisy
# duplicate-UUID warnings during coreaudiod's bundle scan. The build
# rewrites the bundle's Info.plist to this Noican-unique UUID (generated
# once for this project). The HAL plug-in *type* UUID is Apple's and never
# changes; the factory function name stays BlackHole_Create.
UPSTREAM_FACTORY_UUID="e395c745-4eea-4d94-bb92-46224221047c"
NOICAN_FACTORY_UUID="16ccdad9-e4f7-4cd4-8e81-520694b78514"
HAL_PLUGIN_TYPE_UUID="443ABAB8-E7B3-491A-B985-BEB9187030DB"

DIST="$ROOT/dist"
BUILD_DIR="$DIST/driver-build"
DRIVER="$DIST/$DRIVER_NAME.driver"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "This script builds a macOS HAL driver and only runs on macOS" >&2
  exit 1
fi
if ! command -v xcodebuild >/dev/null 2>&1; then
  echo "xcodebuild not found; install Xcode or its Command Line Tools" >&2
  exit 1
fi
if [[ ! -f "$BLACKHOLE/BlackHole.xcodeproj/project.pbxproj" ]]; then
  echo "BlackHole submodule missing; run: git submodule update --init" >&2
  exit 1
fi

# One space-separated build-setting value; xcodebuild splits it into -D
# tokens. \" survives as the C string-literal quote and "\ " keeps names
# with spaces inside a single token (upstream README, "Customizing
# BlackHole"). kLatency_Frame_Size is deliberately absent: v0.7.1 defines
# it without an #ifndef guard, so passing it would redefine the macro.
# The primary device is the visible 1-in/1-out loopback; the mirror stays
# hidden with input and output, like stock BlackHole (explicit for
# documentation).
GCC_DEFS=(
  "kDriver_Name=\\\"$DEVICE_UID_BASE\\\""
  "kHas_Driver_Name_Format=false"
  "kDevice_Name=\\\"${DEVICE_NAME// /\\ }\\\""
  "kDevice2_Name=\\\"${DEVICE2_NAME// /\\ }\\\""
  "kPlugIn_BundleID=\\\"$BUNDLE_ID\\\""
  "kPlugIn_Icon=\\\"$ICON\\\""
  "kManufacturer_Name=\\\"${MANUFACTURER_NAME// /\\ }\\\""
  "kNumber_Of_Channels=$CHANNELS"
  "kSampleRates='$SAMPLE_RATES'"
  "kDevice_IsHidden=false"
  "kDevice_HasInput=true"
  "kDevice_HasOutput=true"
  "kDevice2_IsHidden=true"
  "kDevice2_HasInput=true"
  "kDevice2_HasOutput=true"
)
PREPROCESSOR_DEFS="${GCC_DEFS[*]}"

rm -rf "$BUILD_DIR" "$DRIVER"
mkdir -p "$BUILD_DIR"

# Signing is disabled inside xcodebuild: the bundle's resources are edited
# below, so a seal made here would be broken anyway. One codesign at the
# end signs the final content.
xcodebuild \
  -project "$BLACKHOLE/BlackHole.xcodeproj" \
  -target BlackHole \
  -configuration Release \
  PRODUCT_NAME="$DRIVER_NAME" \
  PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" \
  MARKETING_VERSION="$DRIVER_VERSION" \
  ARCHS="arm64 x86_64" \
  ONLY_ACTIVE_ARCH=NO \
  CODE_SIGNING_ALLOWED=NO \
  CONFIGURATION_BUILD_DIR="$BUILD_DIR" \
  SYMROOT="$BUILD_DIR/sym" \
  OBJROOT="$BUILD_DIR/obj" \
  "GCC_PREPROCESSOR_DEFINITIONS=\$GCC_PREPROCESSOR_DEFINITIONS $PREPROCESSOR_DEFS" \
  build

test -d "$BUILD_DIR/$DRIVER_NAME.driver"
mv "$BUILD_DIR/$DRIVER_NAME.driver" "$DRIVER"

RESOURCES="$DRIVER/Contents/Resources"
# BlackHole branding is an Existential Audio trademark and must not ship
# in the Noican artifact.
rm -f "$RESOURCES/BlackHole.icns"
# GPL-3.0 notice with the source-availability statement for the driver.
cp "$ROOT/LICENSE.driver" "$RESOURCES/LICENSE"
# Optional device icon; without it macOS shows a generic device icon.
if [[ -f "$ROOT/macos/Resources/$ICON" ]]; then
  cp "$ROOT/macos/Resources/$ICON" "$RESOURCES/$ICON"
fi

# Rewrite the factory UUID (see the constant block above for why). This is
# plist-only metadata — a bundle edit, not a source patch. The Delete
# doubles as a guard: if an upstream bump ever changes the plist,
# PlistBuddy fails here instead of silently shipping a stale rewrite.
/usr/libexec/PlistBuddy \
  -c "Delete :CFPlugInFactories:$UPSTREAM_FACTORY_UUID" \
  -c "Add :CFPlugInFactories:$NOICAN_FACTORY_UUID string BlackHole_Create" \
  -c "Set :CFPlugInTypes:$HAL_PLUGIN_TYPE_UUID:0 $NOICAN_FACTORY_UUID" \
  "$DRIVER/Contents/Info.plist"

if [[ -n "${NOICAN_CODESIGN_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp \
    --sign "$NOICAN_CODESIGN_IDENTITY" "$DRIVER"
else
  codesign --force --sign - "$DRIVER"
fi

codesign --verify --deep --strict "$DRIVER"
rm -rf "$BUILD_DIR"
echo "$DRIVER"
