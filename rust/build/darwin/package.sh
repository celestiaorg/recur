#!/bin/sh
# Builds rust/target/package/Recur Next.app, its DMG and the updater zip
# (wails3 task package:rust). Mirrors build/darwin/Taskfile.yml's package,
# create:dmg, sign and sign:notarize for the Go app, under the Rust app's own
# name and bundle id so both install side by side.
#
# Env: VERSION (Info.plist and `recur-cli --version`), UNIVERSAL=true (arm64 +
# x86_64), RECUR_NEXT_UPDATE_URL (the Rust app's signed manifest; never the Go
# app's, whose zip is Recur.app), SIGN_IDENTITY [+ ENTITLEMENTS] for Developer ID
# with KEYCHAIN_PROFILE to notarize, else RECUR_SIGN_IDENTITY or ad-hoc.
set -eu

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
RUST="$ROOT/rust"
OUT="$RUST/target/package"
NAME="Recur Next"
APP="$OUT/$NAME.app"

export MACOSX_DEPLOYMENT_TARGET=12.0
export RECUR_VERSION="${VERSION:?VERSION is required}"
export RECUR_UPDATE_MANIFEST_URL="${RECUR_NEXT_UPDATE_URL:-}"

build() {
  cargo build --manifest-path "$RUST/Cargo.toml" --release --locked -p recur-desktop -p recur-cli "$@"
}

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "${UNIVERSAL:-}" = true ]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  for t in aarch64-apple-darwin x86_64-apple-darwin; do build --target "$t"; done
  for bin in recur recur-cli; do
    lipo -create -output "$APP/Contents/MacOS/$bin" \
      "$RUST/target/aarch64-apple-darwin/release/$bin" "$RUST/target/x86_64-apple-darwin/release/$bin"
  done
else
  build
  cp "$RUST/target/release/recur" "$RUST/target/release/recur-cli" "$APP/Contents/MacOS/"
fi

cp "$ROOT/build/darwin/icons.icns" "$APP/Contents/Resources/"
if [ -f "$ROOT/build/darwin/Assets.car" ]; then cp "$ROOT/build/darwin/Assets.car" "$APP/Contents/Resources/"; fi
cp "$RUST/build/darwin/Info.plist" "$APP/Contents/"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"

# The CLI is signed inside-out before the bundle, as sign:cli does.
if [ -n "${SIGN_IDENTITY:-}" ]; then
  codesign --force --sign "$SIGN_IDENTITY" --timestamp --options runtime \
    ${ENTITLEMENTS:+--entitlements "$ENTITLEMENTS"} "$APP/Contents/MacOS/recur-cli"
  wails3 tool sign --input "$APP" --identity "$SIGN_IDENTITY" \
    ${ENTITLEMENTS:+--entitlements "$ENTITLEMENTS"} \
    ${KEYCHAIN_PROFILE:+--notarize --keychain-profile "$KEYCHAIN_PROFILE"}
else
  ID="${RECUR_SIGN_IDENTITY:--}"
  codesign --force --sign "$ID" "$APP/Contents/MacOS/recur-cli"
  codesign --force --deep --sign "$ID" "$APP"
fi
codesign --verify --deep --strict "$APP"

rm -f "$OUT/$NAME.dmg"
wails3 tool package --format dmg --name "$NAME" --out "$OUT" \
  --background "$ROOT/build/darwin/dmg-background.tiff" --volume-icon "$ROOT/build/darwin/icons.icns"
if [ -n "${SIGN_IDENTITY:-}" ]; then codesign --sign "$SIGN_IDENTITY" --timestamp "$OUT/$NAME.dmg"; fi
if [ -n "${SIGN_IDENTITY:-}" ] && [ -n "${KEYCHAIN_PROFILE:-}" ]; then
  xcrun notarytool submit "$OUT/$NAME.dmg" --keychain-profile "$KEYCHAIN_PROFILE" --wait
  xcrun stapler staple "$OUT/$NAME.dmg"
fi

# The in-app updater swaps the .app in place from this zip, not the DMG.
rm -f "$OUT/$NAME.zip"
ditto -c -k --norsrc --noextattr --keepParent "$APP" "$OUT/$NAME.zip"
echo "$APP"
