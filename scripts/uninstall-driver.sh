#!/usr/bin/env bash
# Removes the Noican driver from /Library/Audio/Plug-Ins/HAL and restarts
# coreaudiod. Requires sudo. The driver keeps no state of its own outside
# the bundle; the HAL "box" name/acquired flags live in coreaudiod's own
# settings store and are inert once the driver is gone (docs/driver.md).
set -euo pipefail

HAL_DIR="/Library/Audio/Plug-Ins/HAL"
DRIVER_DST="$HAL_DIR/Noican.driver"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "HAL drivers are macOS-only" >&2
  exit 1
fi
if [[ ! -d "$DRIVER_DST" ]]; then
  echo "Noican driver is not installed ($DRIVER_DST not found)"
  exit 0
fi
if [[ $EUID -ne 0 ]]; then
  exec sudo "$0" "$@"
fi

rm -rf "$DRIVER_DST"

# See scripts/install-driver.sh for why killall instead of launchctl.
killall coreaudiod 2>/dev/null || true

if [[ -d "$DRIVER_DST" ]]; then
  echo "Failed to remove $DRIVER_DST" >&2
  exit 1
fi

echo "Removed $DRIVER_DST and restarted coreaudiod."
echo "Verify the 'Noican Microphone' device is gone from Audio MIDI Setup."
