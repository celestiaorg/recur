#!/usr/bin/env bash
# Builds Recur from this checkout for this machine and installs it for the
# current user (also `mise run linux:install`).
#
#   build/linux/install.sh [--no-deps]
#
# The build is `mise run linux:build`, the one linux:package and CI use, with
# -C target-cpu=native. This script adds the system packages (deps.sh) and
# copies the result to $RECUR_PREFIX (default ~/.local): lib/recur/ and
# bin/recur. The app writes its own menu entry on first launch.
#
#   --no-deps          skip the system package install
#   RECUR_NATIVE=0     skip -C target-cpu=native (the binary then runs on other CPUs)
#   RECUR_PREFIX=DIR   install somewhere other than ~/.local
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
PREFIX=${RECUR_PREFIX:-$HOME/.local}

deps=1
for arg in "$@"; do
  case $arg in
    --no-deps) deps=0 ;;
    -h | --help) sed -n '2,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

[ "$(uname -s)" = Linux ] || { echo "This is for Linux; on macOS use: mise run macos:package" >&2; exit 1; }
command -v mise >/dev/null 2>&1 || { echo "mise is required (https://mise.jdx.dev); it installs the pinned Rust and tools" >&2; exit 1; }
cd "$ROOT"

[ "$deps" = 0 ] || build/linux/deps.sh

mise install
[ "${RECUR_NATIVE:-1}" = 0 ] || export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=native"
mise run linux:build

src=target/package/recur-linux-$(build/linux/arch.sh)
lib=$PREFIX/lib/recur
mkdir -p "$lib" "$PREFIX/bin"
# Replace files rather than writing into a running binary.
for f in recur recur-cli recur.png; do
  cp "$src/$f" "$lib/$f.new" && mv -f "$lib/$f.new" "$lib/$f"
done
ln -sf "$lib/recur-cli" "$PREFIX/bin/recur"

echo "Installed Recur to $lib"
case ":$PATH:" in
  *":$PREFIX/bin:"*) echo "Run it with: recur ." ;;
  *) echo "Add $PREFIX/bin to your PATH to use 'recur' from a terminal." ;;
esac
