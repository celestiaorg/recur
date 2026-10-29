#!/bin/sh
# Release Linux build of the Rust port (run through in-docker.sh): writes
# rust/target/package/recur-next-linux-<arch>/ with recur (the app), recur-cli
# and recur.png, the flat layout cliinstall and the desktop entry expect.
set -eu

: "${ARCH:?ARCH is required}"
export CARGO_PROFILE_RELEASE_STRIP=symbols
export RECUR_VERSION="${VERSION:?VERSION is required}"
cargo build --release --locked -p recur-desktop -p recur-cli
OUT="target/package/recur-next-linux-$ARCH"
TARGET="${CARGO_TARGET_DIR:-target}/release"
rm -rf "$OUT" && mkdir -p "$OUT"
cp "$TARGET/recur" "$TARGET/recur-cli" "$OUT/"
cp build/appicon.png "$OUT/recur.png"
