#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-apple-darwin"
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

if [[ -n "${NOICAN_CODESIGN_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp \
    --sign "$NOICAN_CODESIGN_IDENTITY" "$APP"
else
  codesign --force --sign - "$APP"
fi

codesign --verify --deep --strict "$APP"
echo "$APP"
