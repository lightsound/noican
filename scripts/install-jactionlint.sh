#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: install-jactionlint.sh <linux-x64|darwin-arm64> <destination>" >&2
  exit 1
fi

readonly platform="$1"
readonly destination="$2"
readonly release_base="https://github.com/jdx/jactionlint/releases/download/v2.3.0"

case "$platform" in
  linux-x64)
    readonly archive_name="jactionlint_2.3.0_linux_amd64.tar.gz"
    readonly archive_size="7325278"
    readonly archive_sha256="907e6f6a59e181879f7e6936e19ee2eca1cbc51de134fb85460cfa8d65a71f28"
    readonly binary_sha256="3566633b33780130c5bea5342ad8c5505092e40094c7d6e25ee74e382899a5c7"
    ;;
  darwin-arm64)
    readonly archive_name="jactionlint_2.3.0_darwin_arm64.tar.gz"
    readonly archive_size="6798574"
    readonly archive_sha256="f96afa36578b21290203ae8df4fa7ac3bf5aba08ed288df28cb0530af7ca6e55"
    readonly binary_sha256="b6853a909613dc9381548b1d61d278e1c4b1ab8334212f67ea6efa46f1f5d34b"
    ;;
  *)
    echo "unsupported jactionlint platform: $platform" >&2
    exit 1
    ;;
esac

temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
archive="$temporary/$archive_name"

curl --fail --silent --show-error --location \
  --proto '=https' --tlsv1.2 \
  "$release_base/$archive_name" \
  --output "$archive"

actual_size="$(wc -c < "$archive" | tr -d '[:space:]')"
if [ "$actual_size" != "$archive_size" ]; then
  echo "jactionlint archive size mismatch: expected $archive_size, received $actual_size" >&2
  exit 1
fi

actual_archive_sha256="$(shasum -a 256 "$archive" | awk '{print $1}')"
if [ "$actual_archive_sha256" != "$archive_sha256" ]; then
  echo "jactionlint archive SHA-256 mismatch" >&2
  exit 1
fi

rm -rf "$destination"
mkdir -p "$destination"
tar -xzf "$archive" -C "$destination" jactionlint

actual_binary_sha256="$(shasum -a 256 "$destination/jactionlint" | awk '{print $1}')"
if [ "$actual_binary_sha256" != "$binary_sha256" ]; then
  echo "jactionlint binary SHA-256 mismatch" >&2
  exit 1
fi

chmod 755 "$destination/jactionlint"
if [ "$("$destination/jactionlint" --version | head -n 1)" != "2.3.0" ]; then
  echo "jactionlint binary version mismatch" >&2
  exit 1
fi
echo "install-jactionlint: verified jactionlint 2.3.0 for $platform"
