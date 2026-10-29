#!/bin/sh
# Smoke test of the Linux packages from linux:package, in a clean Debian 13
# container (mise run linux:smoke). Installs the .deb and checks what it
# owns (nothing of the Go app's `recur` package); then runs the tarball build as
# a user install under Xvfb and D-Bus and checks that it links recur and
# writes its menu entry, that `recur .` opens a repo in the running app,
# and that the boot-time cursor-bridge notice reaches the notification server.
set -eu

if [ "${1:-}" != inside ]; then
  ROOT=$(cd "$(dirname "$0")/../.." && pwd)
  : "${ARCH:?ARCH is required}"
  exec docker run --rm --platform "linux/$ARCH" -e ARCH \
    -v "$ROOT/target/package:/pkg:ro" -v "$ROOT/build/linux/smoke.sh:/smoke.sh:ro" \
    debian:trixie sh /smoke.sh inside
fi

fail() { echo "FAIL: $*"; tail -30 /root/app.out /root/data/logs/*.log 2>/dev/null || true; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends "/pkg/recur-linux-$ARCH.deb" \
  xvfb xauth dbus dbus-x11 dunst libnotify-bin mesa-vulkan-drivers git sqlite3 \
  desktop-file-utils procps >/dev/null

[ "$(readlink -f /usr/bin/recur)" = /opt/recur/recur-cli ] || fail "/usr/bin/recur"
recur --version
desktop-file-validate /usr/share/applications/build.recur.app.desktop
echo "ok: .deb installs to /opt/recur with /usr/bin/recur"

export HOME=/root RECUR_DATA_DIR=/root/data RECUR_WORKTREES_DIR=/root/worktrees PATH="/root/.local/bin:$PATH"
mkdir -p /root/apps /root/data/cursor-bridge/bin /root/repo
tar -xzf "/pkg/recur-linux-$ARCH.tar.gz" -C /root/apps
APP="/root/apps/recur-linux-$ARCH/recur"
printf '#!/bin/sh\n' > /root/data/cursor-bridge/bin/cursor-sdk-bridge
chmod +x /root/data/cursor-bridge/bin/cursor-sdk-bridge
echo '{"sdkVersion":"0.0.1-smoke"}' > /root/data/cursor-bridge/manifest.json
git -C /root/repo init -q -b main
git -C /root/repo -c user.name=Smoke -c user.email=smoke@example.com commit -q --allow-empty -m init

cat > /root/session.sh <<EOF
set -eu
dunst >/dev/null 2>&1 &
dbus-monitor --session "interface='org.freedesktop.Notifications',member='Notify'" > /root/notify.log 2>&1 &
"$APP" > /root/app.out 2>&1 &
PID=\$!
i=0
until [ -L /root/.local/bin/recur ]; do
  i=\$((i + 1)); [ \$i -gt 120 ] && { echo "no recur link"; exit 1; }
  kill -0 \$PID 2>/dev/null || { echo "app exited"; exit 1; }
  sleep 0.5
done
(cd /root/repo && recur .)
i=0
until [ -n "\$(sqlite3 /root/data/recur.db "select id from projects where repo_path = '/root/repo'" 2>/dev/null)" ]; do
  i=\$((i + 1)); [ \$i -gt 40 ] && { echo "recur . did not open /root/repo"; exit 1; }
  sleep 0.5
done
sleep 2
kill -0 \$PID 2>/dev/null || { echo "app exited"; exit 1; }
kill \$PID
EOF
xvfb-run -a dbus-run-session -- sh /root/session.sh || fail "session"
[ "$(readlink /root/.local/bin/recur)" = "/root/apps/recur-linux-$ARCH/recur-cli" ] || fail "link target"
echo "ok: tarball install linked ~/.local/bin/recur"
grep -q "^Exec=\"$APP\"" /root/.local/share/applications/build.recur.app.desktop || fail "menu entry"
desktop-file-validate /root/.local/share/applications/build.recur.app.desktop
echo "ok: menu entry ~/.local/share/applications/build.recur.app.desktop"
echo "ok: recur . opened /root/repo"
grep -q 'Cursor bridge update available' /root/notify.log || fail "no notification (notify.log: $(cat /root/notify.log))"
echo "ok: notification delivered over org.freedesktop.Notifications"
echo "ok: Linux package smoke passed"
