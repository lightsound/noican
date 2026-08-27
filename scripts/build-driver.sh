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
#   device UID      "com.lightsound.noican.2ch_UID"
#   channels        2, sample rates 44.1/48 kHz (Noican pins 48 kHz)
#
# The device UID cannot be set directly: BlackHole derives kDevice_UID as
# kDriver_Name "_UID" (with kHas_Driver_Name_Format=false), so kDriver_Name
# carries the reverse-DNS UID base. kDriver_Name is not user-visible; the
# visible strings are kDevice_Name and kManufacturer_Name. The resulting UID
# matches both existing matchers verbatim: Swift
# AudioDeviceCatalog.isNoicanVirtualDevice and Rust is_noican_loopback_uid
# (case-insensitive "com.lightsound.noican." prefix).
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
DEVICE_UID_BASE="com.lightsound.noican.2ch"
DEVICE_NAME="Noican Microphone"
DEVICE2_NAME="Noican Microphone Mirror"
MANUFACTURER_NAME="lightsound"
ICON="Noican.icns"
CHANNELS=2
SAMPLE_RATES="44100,48000"
DRIVER_VERSION="${NOICAN_DRIVER_VERSION:-0.1.0}"

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
PREPROCESSOR_DEFS="kDriver_Name=\\\"$DEVICE_UID_BASE\\\""
PREPROCESSOR_DEFS+=" kHas_Driver_Name_Format=false"
PREPROCESSOR_DEFS+=" kDevice_Name=\\\"${DEVICE_NAME// /\\ }\\\""
PREPROCESSOR_DEFS+=" kDevice2_Name=\\\"${DEVICE2_NAME// /\\ }\\\""
PREPROCESSOR_DEFS+=" kPlugIn_BundleID=\\\"$BUNDLE_ID\\\""
PREPROCESSOR_DEFS+=" kPlugIn_Icon=\\\"$ICON\\\""
PREPROCESSOR_DEFS+=" kManufacturer_Name=\\\"${MANUFACTURER_NAME// /\\ }\\\""
PREPROCESSOR_DEFS+=" kNumber_Of_Channels=$CHANNELS"
PREPROCESSOR_DEFS+=" kSampleRates='$SAMPLE_RATES'"
# The primary device is the visible 2-in/2-out loopback; the mirror stays
# hidden, exactly like stock BlackHole 2ch (explicit for documentation).
PREPROCESSOR_DEFS+=" kDevice_IsHidden=false"
PREPROCESSOR_DEFS+=" kDevice_HasInput=true"
PREPROCESSOR_DEFS+=" kDevice_HasOutput=true"
PREPROCESSOR_DEFS+=" kDevice2_IsHidden=true"
PREPROCESSOR_DEFS+=" kDevice2_HasInput=true"
PREPROCESSOR_DEFS+=" kDevice2_HasOutput=true"

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

# CFPlugIn factory UUIDs must be unique per plug-in, but upstream hardcodes
# one in BlackHole.plist, so every unpatched BlackHole fork (stock BlackHole,
# JoyCast, ...) ships the same UUID and coexisting forks trigger harmless but
# noisy duplicate-UUID warnings during coreaudiod's bundle scan. Rewrite the
# built bundle's Info.plist to a Noican-unique factory UUID (generated once
# for this project). The factory *function* stays BlackHole_Create — the
# UUID is plist-only metadata, so this is a bundle edit, not a source patch.
# The Delete doubles as a guard: if an upstream bump ever changes the plist,
# PlistBuddy fails here instead of silently shipping a stale rewrite.
UPSTREAM_FACTORY_UUID="e395c745-4eea-4d94-bb92-46224221047c"
NOICAN_FACTORY_UUID="16ccdad9-e4f7-4cd4-8e81-520694b78514"
HAL_PLUGIN_TYPE_UUID="443ABAB8-E7B3-491A-B985-BEB9187030DB"
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
