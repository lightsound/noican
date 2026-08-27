#!/usr/bin/env bash
# Installs dist/Noican.driver into /Library/Audio/Plug-Ins/HAL and restarts
# coreaudiod so it loads. Requires sudo. Build the driver first with
# scripts/build-driver.sh; macOS 15+ coreaudiod only loads Developer-ID
# signed drivers, so install a NOICAN_CODESIGN_IDENTITY build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DRIVER_SRC="${1:-$ROOT/dist/Noican.driver}"
HAL_DIR="/Library/Audio/Plug-Ins/HAL"
DRIVER_DST="$HAL_DIR/Noican.driver"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "HAL drivers are macOS-only" >&2
  exit 1
fi
if [[ ! -d "$DRIVER_SRC" ]]; then
  echo "Driver bundle not found at $DRIVER_SRC" >&2
  echo "Build it first: bash scripts/build-driver.sh" >&2
  exit 1
fi
if [[ $EUID -ne 0 ]]; then
  exec sudo "$0" "$DRIVER_SRC"
fi

rm -rf "$DRIVER_DST"
# ditto preserves the bundle structure and the code signature.
ditto "$DRIVER_SRC" "$DRIVER_DST"
chown -R root:wheel "$DRIVER_DST"
chmod -R go-w "$DRIVER_DST"

# On macOS 26, SIP rejects `launchctl kickstart -k
# system/com.apple.audio.coreaudiod`; killing the daemon (it restarts on
# demand) is the working restart path (docs/macos-hardware-test.md).
killall coreaudiod 2>/dev/null || true

echo "Installed $DRIVER_DST and restarted coreaudiod."
echo "Verify in Audio MIDI Setup: 'Noican Microphone' (2 ch, 48 kHz)."
