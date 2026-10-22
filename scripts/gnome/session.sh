#!/usr/bin/env bash
# Inside `dbus-run-session`, as the test user: starts PipeWire, a headless GNOME
# Shell (Ubuntu's tray extension on, plus a small test extension that turns on
# the shell's unsafe mode so the test can press keys, read the panel and take
# screenshots), then the portals, then runs "$@" and stops everything.
# Logs go to $LOGS (default /tmp/gnome-logs).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
export LOGS=${LOGS:-/tmp/gnome-logs}
mkdir -p "$LOGS"
export XDG_SESSION_ID=c1 XDG_CURRENT_DESKTOP=ubuntu:GNOME XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=ubuntu
export GSETTINGS_BACKEND=dconf NO_AT_BRIDGE=0 GTK_A11Y=atspi

ext=$HOME/.local/share/gnome-shell/extensions/caw-test@birdbrainlabs.dev
mkdir -p "$ext"
cp "$here/test-extension/"* "$ext/"
tray=$(ls /usr/share/gnome-shell/extensions | grep -i appindicator | head -1)
gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.shell enabled-extensions "['$tray', 'caw-test@birdbrainlabs.dev']"
gsettings set org.gnome.desktop.interface toolkit-accessibility true
echo "tray extension: $tray"

# What the shell and anything D-Bus starts for it see (the shell itself must not have WAYLAND_DISPLAY).
dbus-update-activation-environment WAYLAND_DISPLAY=wayland-0 DISPLAY=:0 XDG_CURRENT_DESKTOP XDG_SESSION_TYPE XDG_SESSION_DESKTOP GTK_A11Y XDG_RUNTIME_DIR 2>/dev/null || true

pipewire >"$LOGS/pipewire.log" 2>&1 &
sleep 0.5
wireplumber >"$LOGS/wireplumber.log" 2>&1 &
pipewire-pulse >"$LOGS/pipewire-pulse.log" 2>&1 &

gnome-shell --headless --wayland --virtual-monitor 1280x800 >"$LOGS/gnome-shell.log" 2>&1 &
shell=$!
for _ in $(seq 60); do
  [ -S "$XDG_RUNTIME_DIR/wayland-0" ] && break
  sleep 0.5
done
if ! [ -S "$XDG_RUNTIME_DIR/wayland-0" ]; then
  echo "GNOME Shell did not start"; cat "$LOGS/gnome-shell.log"; exit 1
fi
export WAYLAND_DISPLAY=wayland-0 DISPLAY=:0
# The shell is up once it answers on D-Bus.
for _ in $(seq 60); do
  gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell -m org.freedesktop.DBus.Properties.Get org.gnome.Shell ShellVersion >/dev/null 2>&1 && break
  sleep 0.5
done
if ! kill -0 $shell 2>/dev/null; then
  echo "GNOME Shell stopped"; cat "$LOGS/gnome-shell.log"; exit 1
fi
sleep 3
# Portals the shell started before it was ready can't show dialogs: start them afresh.
pkill -u "$(id -u)" -f 'libexec/xdg-desktop-portal' 2>/dev/null
sleep 0.5
/usr/libexec/xdg-desktop-portal-gnome >"$LOGS/portal-gnome.log" 2>&1 &
sleep 1
/usr/libexec/xdg-desktop-portal -v >"$LOGS/portal.log" 2>&1 &
sleep 2

"$@"
status=$?

kill %1 %2 %3 $shell 2>/dev/null
pkill -u "$(id -u)" -f xdg-desktop-portal 2>/dev/null
exit $status
