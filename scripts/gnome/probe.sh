#!/usr/bin/env bash
# Probe (inside session.sh): what this GNOME offers Caw.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
echo "== portal interfaces"
gdbus introspect --session -d org.freedesktop.portal.Desktop -o /org/freedesktop/portal/desktop | grep -o 'interface org.freedesktop[a-zA-Z.]*' | sort -u
echo "== shell eval (unsafe mode)"
python3 -c 'from gnometest import *; print(shell_eval("global.context.unsafe_mode")); print(windows())'
echo "== out of the overview"
python3 -c 'from gnometest import *; print(calm_shell()); import time; time.sleep(1); print(shell_eval("[Main.overview.visible, Main.actionMode, Main.modalCount]"))'
echo "== mutter keybindings with injected keys"
python3 mutter_keys.py
echo "== tray watcher"
gdbus call --session -d org.kde.StatusNotifierWatcher -o /StatusNotifierWatcher -m org.freedesktop.DBus.Properties.Get org.kde.StatusNotifierWatcher IsStatusNotifierHostRegistered || true
mkdir -p ~/.local/share/applications
printf '[Desktop Entry]\nType=Application\nName=Caw\nExec=true\n' > ~/.local/share/applications/dev.birdbrainlabs.CawDesktop.desktop
echo "== global shortcuts, no app id"
timeout 120 python3 probe_shortcuts.py Control_R CTRL+ALT+k
echo "== global shortcuts, registered app id"
timeout 120 python3 probe_shortcuts.py --register dev.birdbrainlabs.CawDesktop Control_R CTRL+ALT+k
echo "== remote desktop (typing), registered"
timeout 60 python3 probe_remote.py --register dev.birdbrainlabs.CawDesktop
echo "== logs"
for f in portal-gnome portal gnome-shell; do echo "-- $f"; tail -n 80 "$LOGS/$f.log"; done
