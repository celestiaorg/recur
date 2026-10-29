#!/bin/sh
# Runs a command from the repo root in the Linux build image (build/linux/Dockerfile),
# or directly when already on Linux. ARCH (amd64 / arm64, default the host's)
# picks the image platform; on a Mac anything but the host arch runs emulated.
# The cargo registry and target dir live in Docker volumes, so target on
# the Mac is never touched except for what the command copies there.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ARCH=$("$ROOT/build/linux/arch.sh")
export ARCH

if [ "$(uname -s)" = Linux ]; then
  cd "$ROOT" && exec "$@"
fi
docker info >/dev/null 2>&1 || { echo "Docker (or OrbStack) is required to build for Linux" >&2; exit 1; }
# One job per 2 GB of VM memory: test binaries link in parallel and each ld
# holds a few GB, which a Docker VM with a core per GB runs out of.
JOBS=$(( $(docker info --format '{{.MemTotal}}') / 2147483648 ))
IMAGE="recur-linux-build:$ARCH"
docker build -q --platform "linux/$ARCH" -t "$IMAGE" -f "$ROOT/build/linux/Dockerfile" "$ROOT/build/linux" >/dev/null
# WPE=1: the image plus WPE WebKit 2.54 (wpe.Dockerfile, the HTML preview spike).
if [ "${WPE:-}" = 1 ]; then
  docker build -q --platform "linux/$ARCH" --build-arg ARCH="$ARCH" -t "recur-linux-wpe:$ARCH" \
    -f "$ROOT/build/linux/wpe.Dockerfile" "$ROOT/build/linux" >/dev/null
  IMAGE="recur-linux-wpe:$ARCH"
fi
exec docker run --rm --init --platform "linux/$ARCH" \
  -v "$ROOT:/app" \
  -v "recur-linux-cargo-$ARCH:/usr/local/cargo/registry" \
  -v "recur-linux-target-$ARCH:/target" \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS="$JOBS" -e ARCH -e VERSION -e RECUR_UPDATE_URL -e WPE_NETWORK \
  "$IMAGE" "$@"
