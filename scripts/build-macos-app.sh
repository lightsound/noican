#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-apple-darwin"
# Match the Swift package's platform (.macOS(.v14)). Without this, cc-built
# objects in the Rust staticlib (ring, tract-linalg assembly) default to the
# host SDK version and ld warns that they target a newer macOS than the app.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
# CONFIGURATION (release|debug) selects the Swift build configuration only.
# The Rust staticlib is always built in release: Package.swift links
# ../target/aarch64-apple-darwin/release, and a debug-profile engine is too
# slow for the real-time path anyway.
CONFIGURATION="${CONFIGURATION:-release}"
case "$CONFIGURATION" in
  release|debug) ;;
  *) echo "CONFIGURATION must be 'release' or 'debug', got '$CONFIGURATION'" >&2; exit 1 ;;
esac
APP="$ROOT/dist/Noican.app"
ENTITLEMENTS="$ROOT/macos/Resources/Noican.entitlements"
# The one entitlement the app cannot run without: under the hardened
# runtime (Developer ID signature below) tccd denies the microphone
# silently unless the signature carries it. Checked after signing.
AUDIO_INPUT_ENTITLEMENT="com.apple.security.device.audio-input"

cargo build \
  --manifest-path "$ROOT/Cargo.toml" \
  --locked \
  --package noican-ffi \
  --release \
  --target "$TARGET"

# Warnings are errors, matching the Rust side of the quality gates.
swift build \
  --package-path "$ROOT/macos" \
  --configuration "$CONFIGURATION" \
  --arch arm64 \
  -Xswiftc -warnings-as-errors

SWIFT_BINARY="$ROOT/macos/.build/arm64-apple-macosx/$CONFIGURATION/NoicanMenuBar"
test -x "$SWIFT_BINARY"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$SWIFT_BINARY" "$APP/Contents/MacOS/NoicanMenuBar"
cp "$ROOT/macos/Resources/Info.plist" "$APP/Contents/Info.plist"

# Both signatures carry the same entitlements, so the Developer ID and
# the ad-hoc build differ only in identity, hardened runtime, and
# timestamp — and the CI ad-hoc build catches a broken entitlements
# plist. The ad-hoc signature has no hardened runtime, so TCC prompts
# there with or without the entitlement; the Developer ID signature
# does not work without it (see the plist).
if [[ -n "${NOICAN_CODESIGN_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$NOICAN_CODESIGN_IDENTITY" "$APP"
else
  codesign --force --entitlements "$ENTITLEMENTS" --sign - "$APP"
fi

codesign --verify --deep --strict "$APP"

# Fail if the entitlement did not make it into the signature: a
# Developer ID bundle without it launches, logs a healthy engine, and
# never captures a sample (tccd: "requires entitlement
# com.apple.security.device.audio-input but it is missing").
#
# plutil key paths are dot-separated, so the dots in the key are escaped.
if ! codesign --display --entitlements - --xml "$APP" 2>/dev/null \
  | plutil -extract "${AUDIO_INPUT_ENTITLEMENT//./\\.}" raw -o - - 2>/dev/null \
  | grep -qx true; then
  echo "error: $APP is signed without $AUDIO_INPUT_ENTITLEMENT = true" >&2
  echo "       (inspect with: codesign --display --entitlements - --xml $APP)" >&2
  exit 1
fi
echo "$APP"
