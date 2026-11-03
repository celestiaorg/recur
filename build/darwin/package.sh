#!/bin/sh
# Builds target/package/Recur.app, its DMG and the updater zip
# (mise run macos:package).
#
# Env: VERSION (Info.plist and `recur-cli --version`; defaults to
# build/version.sh), UNIVERSAL=true (arm64 + x86_64), RECUR_UPDATE_URL
# (the signed update manifest), SIGN_IDENTITY [+ ENTITLEMENTS] for Developer ID
# with KEYCHAIN_PROFILE (an `xcrun notarytool store-credentials` profile) to
# notarize, else RECUR_SIGN_IDENTITY or ad-hoc.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT="$ROOT/target/package"
NAME="Recur"
APP="$OUT/$NAME.app"
# rustup reads rust-toolchain.toml (the pinned toolchain with llvm-tools) from the cwd.
cd "$ROOT"

export MACOSX_DEPLOYMENT_TARGET=12.0
export CARGO_PROFILE_RELEASE_STRIP=symbols
VERSION="${VERSION:-$(build/version.sh)}"
export RECUR_VERSION="$VERSION"
export RECUR_UPDATE_MANIFEST_URL="${RECUR_UPDATE_URL:-}"
# Settings > Licenses (mise run generate:licenses).
export RECUR_LICENSES_MD="$ROOT/target/licenses.md"

build() {
  cargo build --release --locked -p recur-desktop -p recur-cli "$@"
}

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "${UNIVERSAL:-}" = true ]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  # The Intel slice builds libvpx and libaom from source with their x86 assembly.
  command -v nasm >/dev/null || brew install nasm
  for t in aarch64-apple-darwin x86_64-apple-darwin; do build --target "$t"; done
  for bin in recur recur-cli; do
    lipo -create -output "$APP/Contents/MacOS/$bin" \
      "$ROOT/target/aarch64-apple-darwin/release/$bin" "$ROOT/target/x86_64-apple-darwin/release/$bin"
  done
else
  build
  cp "$ROOT/target/release/recur" "$ROOT/target/release/recur-cli" "$APP/Contents/MacOS/"
fi

cp "$ROOT/build/darwin/icons.icns" "$APP/Contents/Resources/"
if [ -f "$ROOT/build/darwin/Assets.car" ]; then cp "$ROOT/build/darwin/Assets.car" "$APP/Contents/Resources/"; fi
cp "$ROOT/build/darwin/Info.plist" "$APP/Contents/"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"

# Signed inside-out: the CLI, then the bundle (which seals the app binary).
if [ -n "${SIGN_IDENTITY:-}" ]; then
  for target in "$APP/Contents/MacOS/recur-cli" "$APP"; do
    codesign --force --sign "$SIGN_IDENTITY" --timestamp --options runtime \
      ${ENTITLEMENTS:+--entitlements "$ENTITLEMENTS"} "$target"
  done
  if [ -n "${KEYCHAIN_PROFILE:-}" ]; then
    ditto -c -k --keepParent "$APP" "$OUT/notarize.zip"
    xcrun notarytool submit "$OUT/notarize.zip" --keychain-profile "$KEYCHAIN_PROFILE" --wait
    rm -f "$OUT/notarize.zip"
    xcrun stapler staple "$APP"
  fi
else
  ID="${RECUR_SIGN_IDENTITY:--}"
  codesign --force --sign "$ID" "$APP/Contents/MacOS/recur-cli"
  codesign --force --deep --sign "$ID" "$APP"
fi
codesign --verify --deep --strict "$APP"

rm -f "$OUT/$NAME.dmg"
dmgbuild -s build/darwin/dmg-settings.py -D app="$APP" \
  -D background=build/darwin/dmg-background.tiff -D volume_icon=build/darwin/icons.icns \
  "$NAME" "$OUT/$NAME.dmg"
if [ -n "${SIGN_IDENTITY:-}" ]; then codesign --sign "$SIGN_IDENTITY" --timestamp "$OUT/$NAME.dmg"; fi
if [ -n "${SIGN_IDENTITY:-}" ] && [ -n "${KEYCHAIN_PROFILE:-}" ]; then
  xcrun notarytool submit "$OUT/$NAME.dmg" --keychain-profile "$KEYCHAIN_PROFILE" --wait
  xcrun stapler staple "$OUT/$NAME.dmg"
fi

# The in-app updater swaps the .app in place from this zip, not the DMG.
rm -f "$OUT/$NAME.zip"
ditto -c -k --norsrc --noextattr --keepParent "$APP" "$OUT/$NAME.zip"
echo "$APP"
