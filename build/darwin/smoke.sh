#!/bin/sh
# Smoke test of the packaged Recur DMG (mise run smoke:package):
# installs the app from it into a sandbox (a build under target counts as
# a dev build and never links the CLI), launches it on a throwaway HOME and
# RECUR_DATA_DIR, checks that it links
# `recur` onto PATH, that `recur .` opens a repo in the running app,
# and that the boot-time cursor-bridge notice goes out as a native notification.
set -eu

DMG="${1:-target/package/Recur.dmg}"
# Short: the open-IPC socket path in the data dir must fit in sun_path.
SB=/tmp/rn-smoke.noindex
rm -rf "$SB"
mkdir -p "$SB/home" "$SB/data" "$SB/repo" "$SB/Applications"
SB=$(cd "$SB" && pwd -P)
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$SB/dmg" "$DMG"
ditto "$SB/dmg/Recur.app" "$SB/Applications/Recur.app"
hdiutil detach -quiet "$SB/dmg"
APP="$SB/Applications/Recur.app"
codesign --verify --deep --strict "$APP"
export HOME="$SB/home" RECUR_DATA_DIR="$SB/data" RECUR_WORKTREES_DIR="$SB/worktrees"

fail() { echo "FAIL: $*"; echo "--- app log"; tail -40 "$SB/app.out" "$SB/data/logs/"*.log 2>/dev/null; exit 1; }

# A cursor-bridge off the pinned SDK makes the app post a notification at boot.
mkdir -p "$SB/data/cursor-bridge/bin"
printf '#!/bin/sh\n' > "$SB/data/cursor-bridge/bin/cursor-sdk-bridge"
chmod +x "$SB/data/cursor-bridge/bin/cursor-sdk-bridge"
echo '{"sdkVersion":"0.0.1-smoke"}' > "$SB/data/cursor-bridge/manifest.json"

git -C "$SB/repo" init -q -b main
echo smoke > "$SB/repo/README.md"
git -C "$SB/repo" add README.md
git -C "$SB/repo" -c user.name=Smoke -c user.email=smoke@example.com commit -qm init

START=$(date '+%Y-%m-%d %H:%M:%S')
# Through LaunchServices, as Finder and the Dock launch it: usernoted only
# accepts notification requests from a registered bundle.
open -n -g --stdout "$SB/app.out" --stderr "$SB/app.out" \
  --env HOME="$HOME" --env RECUR_DATA_DIR="$RECUR_DATA_DIR" --env RECUR_WORKTREES_DIR="$RECUR_WORKTREES_DIR" "$APP"
i=0
until PID=$(pgrep -f "^$APP/Contents/MacOS/recur"); do
  i=$((i + 1)); [ $i -gt 40 ] && fail "app did not start"
  sleep 0.25
done
trap 'kill $PID 2>/dev/null || true' EXIT

LINK="$HOME/.local/bin/recur"
i=0
until [ -L "$LINK" ]; do
  i=$((i + 1)); [ $i -gt 120 ] && fail "no $LINK after 60s"
  kill -0 $PID 2>/dev/null || fail "app exited"
  sleep 0.5
done
[ "$(readlink "$LINK")" = "$APP/Contents/MacOS/recur-cli" ] || fail "$LINK -> $(readlink "$LINK")"
echo "ok: $LINK -> $(readlink "$LINK")"

(cd "$SB/repo" && "$LINK" .) || fail "recur . failed"
i=0
until [ -n "$(sqlite3 "$SB/data/recur.db" "select id from projects where repo_path = '$SB/repo'" 2>/dev/null)" ]; do
  i=$((i + 1)); [ $i -gt 40 ] && fail "recur . did not add $SB/repo"
  sleep 0.5
done
echo "ok: recur . opened $SB/repo"

# Native delivery logs nothing in the app (the osascript fallback logs only
# on failure), so read usernoted's record of the request.
sleep 3
kill -0 $PID 2>/dev/null || fail "app exited"
/usr/bin/log show --start "$START" --style compact \
  --predicate 'process == "usernoted" AND eventMessage CONTAINS "app:\"build.recur.app\""' > "$SB/usernoted.log" 2>&1 || true
grep -q 'Delivering <NotificationRecord app:"build.recur.app"' "$SB/usernoted.log" ||
  fail "usernoted delivered no notification from build.recur.app"
echo "ok: native notification delivered by build.recur.app"
if grep -q 'Presenting <NotificationRecord app:"build.recur.app".* as none' "$SB/usernoted.log"; then
  echo "note: not shown on screen until Recur is allowed in System Settings > Notifications"
fi
grep -i 'notification' "$SB/app.out" "$SB/data/logs/"*.log 2>/dev/null && fail "notification errors logged"
echo "ok: package smoke passed"
