#!/usr/bin/env bash
# Builds the Rust engine, the SwiftUI app, and assembles Noican.app.
# Run from the repository root on macOS (Apple Silicon).
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> Building Rust FFI library (release)"
cargo build --release -p noican-ffi

echo "==> Building Swift app"
swift build -c release --package-path app

echo "==> Assembling Noican.app"
APP=build/Noican.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp app/.build/release/NoicanApp "$APP/Contents/MacOS/NoicanApp"
cp app/Info.plist "$APP/Contents/Info.plist"

# Ad-hoc signature is enough for local use; the mic TCC prompt needs a
# stable identity, which ad-hoc provides per-binary.
codesign --force --sign - "$APP"

echo "==> Done: $APP"
echo "    open $APP    (grant the microphone permission on first start)"
