#!/bin/sh
# Starts a virtual X desktop, exposes it on :6080 via noVNC, then runs Recur.
# The container exits when Recur quits.
set -e
export DISPLAY=:1
Xvfb :1 -screen 0 "${SCREEN:-1600x1000x24}" -nolisten tcp &
until [ -e /tmp/.X11-unix/X1 ]; do sleep 0.1; done
eval "$(dbus-launch --sh-syntax)"
openbox &
dunst >/dev/null 2>&1 &
x11vnc -display :1 -forever -shared -nopw -quiet -rfbport 5900 >/dev/null 2>&1 &
websockify --web /usr/share/novnc 6080 localhost:5900 >/dev/null 2>&1 &
exec /opt/recur/recur
