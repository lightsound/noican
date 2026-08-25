#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-apple-darwin"
CONFIGURATION="${CONFIGURATION:-release}"
APP="$ROOT/dist/noican.app"

cargo build \
  --manifest-path "$ROOT/Cargo.toml" \
  --locked \
  --package noican-ffi \
  --profile "$CONFIGURATION" \
  --target "$TARGET"

swift build \
  --package-path "$ROOT/macos" \
  --configuration "$CONFIGURATION" \
  --arch arm64

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
