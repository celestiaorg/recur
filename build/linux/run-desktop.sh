#!/bin/sh
# Runs the Linux build in target/package/recur-next-linux-<arch> on a
# throwaway Linux desktop served to the browser by noVNC (wails3 task
# linux:run:rust). Home persists in the recur-next-linux-home-<arch> volume
# (`docker volume rm` it to reset to first run); Ctrl+C or File > Quit stops it.
# VNC_PORT defaults to 6081 so it can run beside the Go app's (6080).
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
: "${ARCH:?ARCH is required}"
PORT="${VNC_PORT:-6081}"
IMAGE="recur-next-linux-desktop:$ARCH"
docker build -q --platform "linux/$ARCH" -t "$IMAGE" -f "$ROOT/build/linux/Dockerfile.desktop" "$ROOT/build/linux" >/dev/null
docker rm -f recur-next-linux >/dev/null 2>&1 || true
URL="http://127.0.0.1:$PORT/vnc.html?autoconnect=1&resize=scale"
echo "Recur Next (Linux) desktop: $URL"
(for i in $(seq 120); do curl -fs "$URL" >/dev/null 2>&1 && break; sleep 0.5; done; open "$URL" 2>/dev/null || xdg-open "$URL" 2>/dev/null) &
exec docker run --rm --init --name recur-next-linux --platform "linux/$ARCH" \
  -p "127.0.0.1:$PORT:6080" \
  -v "recur-next-linux-home-$ARCH:/home/dev" \
  -v "$ROOT/target/package/recur-next-linux-$ARCH:/srv/recur:ro" \
  "$IMAGE"
