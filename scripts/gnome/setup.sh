#!/usr/bin/env bash
# Prepares an Ubuntu container (run as root) for a headless GNOME session:
# GNOME Shell, the portals, the Ubuntu tray extension, PipeWire with a fake
# microphone, AT-SPI (to read and press dialog buttons), and a test user.
# Usage: scripts/gnome/setup.sh
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
. /etc/os-release
echo "== $PRETTY_NAME"
apt-get install -y -qq --no-install-recommends \
  gnome-shell mutter xwayland xdg-desktop-portal xdg-desktop-portal-gnome gnome-shell-extension-appindicator \
  dbus dbus-user-session dconf-service dconf-cli gsettings-desktop-schemas \
  libgl1-mesa-dri libegl-mesa0 mesa-vulkan-drivers \
  pipewire pipewire-pulse pipewire-alsa wireplumber pulseaudio-utils \
  at-spi2-core gir1.2-atspi-2.0 python3-gi gir1.2-gtk-4.0 gir1.2-gdkpixbuf-2.0 \
  adwaita-icon-theme fonts-cantarell ca-certificates curl bzip2 procps psmisc \
  >/dev/null
echo "== versions"
gnome-shell --version || true
dpkg-query -W -f='${Package} ${Version}\n' gnome-shell mutter xdg-desktop-portal xdg-desktop-portal-gnome gnome-shell-extension-appindicator libgtk-4-1 2>/dev/null || true
ls /usr/share/gnome-shell/extensions/
id tester >/dev/null 2>&1 || useradd -m -u 1500 -s /bin/bash tester
mkdir -p /run/user/1500 && chown tester:tester /run/user/1500 && chmod 700 /run/user/1500
# Xwayland's socket folder; a system bus with a stand-in logind (GNOME Shell
# won't start without one, and a container has neither).
mkdir -p /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p /etc/dbus-1/system.d /run/dbus
cat >/etc/dbus-1/system.d/caw-test-logind.conf <<'CONF'
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <policy user="root"><allow own="org.freedesktop.login1"/></policy>
  <policy context="default">
    <allow send_destination="org.freedesktop.login1"/>
    <allow receive_sender="org.freedesktop.login1"/>
  </policy>
</busconfig>
CONF
[ -S /run/dbus/system_bus_socket ] || dbus-daemon --system --fork
python3 "$here/fake-logind.py" 1500 tester >/tmp/fake-logind.log 2>&1 &
sleep 1
cat /tmp/fake-logind.log
