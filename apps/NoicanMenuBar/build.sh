#!/usr/bin/env bash
#
# Builds the Rust engine and the menu bar app, then assembles a .app bundle.
#
# A bundle rather than a bare executable because macOS needs one for two
# things a menu bar app cannot do without: LSUIElement, which keeps it out of
# the Dock, and NSMicrophoneUsageDescription, without which the microphone TCC
# prompt never appears and capture silently returns zeros.

set -euo pipefail

CONFIGURATION="${1:-release}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../.." && pwd)"
BUNDLE="$HERE/.build/NoicanMenuBar.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this app only builds on macOS" >&2
  exit 1
fi

echo "==> Building the Rust engine ($CONFIGURATION)"
if [[ "$CONFIGURATION" == "release" ]]; then
  cargo build --manifest-path "$WORKSPACE/Cargo.toml" -p noican-ffi --release
  RUST_LIB_DIR="$WORKSPACE/target/release"
else
  cargo build --manifest-path "$WORKSPACE/Cargo.toml" -p noican-ffi
  RUST_LIB_DIR="$WORKSPACE/target/debug"
fi

echo "==> Building the app"
export NOICAN_RUST_LIB_DIR="$RUST_LIB_DIR"
swift build --package-path "$HERE" -c "$CONFIGURATION"

BINARY="$(swift build --package-path "$HERE" -c "$CONFIGURATION" --show-bin-path)/NoicanMenuBar"

echo "==> Assembling $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$BINARY" "$BUNDLE/Contents/MacOS/NoicanMenuBar"
cp "$HERE/Info.plist" "$BUNDLE/Contents/Info.plist"

# Ad-hoc signing is enough for the app itself during development. The virtual
# audio driver is the part that genuinely requires a Developer ID, because
# coreaudiod refuses to load anything less (docs/tech-research.md section 3.1).
echo "==> Signing (ad-hoc)"
codesign --force --sign - --entitlements "$HERE/NoicanMenuBar.entitlements" \
  --options runtime "$BUNDLE"

echo
echo "Built $BUNDLE"
echo "Run it with: open '$BUNDLE'"
