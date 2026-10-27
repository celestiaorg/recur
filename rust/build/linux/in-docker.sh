#!/bin/sh
# Runs a command from rust/ in the Linux build image (rust/build/linux/Dockerfile),
# or directly when already on Linux. ARCH (amd64 / arm64, default the host's)
# picks the image platform; on a Mac anything but the host arch runs emulated.
# The cargo registry and target dir live in Docker volumes, so rust/target on
# the Mac is never touched except for what the command copies there.
set -eu

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
case "${ARCH:-$(uname -m)}" in
  x86_64 | amd64) ARCH=amd64 ;;
  aarch64 | arm64) ARCH=arm64 ;;
  *) echo "unsupported arch ${ARCH:-$(uname -m)}" >&2; exit 1 ;;
esac
export ARCH

if [ "$(uname -s)" = Linux ]; then
  cd "$ROOT/rust" && exec "$@"
fi
docker info >/dev/null 2>&1 || { echo "Docker (or OrbStack) is required to build for Linux" >&2; exit 1; }
IMAGE="recur-next-linux-build:$ARCH"
docker build -q --platform "linux/$ARCH" -t "$IMAGE" -f "$ROOT/rust/build/linux/Dockerfile" "$ROOT/rust/build/linux" >/dev/null
exec docker run --rm --platform "linux/$ARCH" \
  -v "$ROOT:/app" \
  -v "recur-next-cargo-$ARCH:/usr/local/cargo/registry" \
  -v "recur-next-target-$ARCH:/target" \
  -e CARGO_TARGET_DIR=/target -e ARCH -e VERSION -e RECUR_NEXT_UPDATE_URL \
  "$IMAGE" "$@"
