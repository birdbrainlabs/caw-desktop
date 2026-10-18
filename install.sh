#!/bin/sh
# Installs or upgrades Caw Desktop for the current user on Linux, without sudo:
#
#   curl -fsSL https://raw.githubusercontent.com/birdbrainlabs/caw-desktop/master/install.sh | sh
#
# Puts caw-desktop in ~/.local/bin, starts it now and at every login with a
# systemd user service, then shows the QR code for pairing your phone.
# Running it again upgrades in place and restarts the service.
#
# CAW_INSTALL_URL=<url>  install from another tar.gz (a file:// URL works too)
# CAW_NO_START=1         only install the files; don't touch the service or pair
set -eu

repo=birdbrainlabs/caw-desktop
url=${CAW_INSTALL_URL:-https://github.com/$repo/releases/latest/download/caw-desktop-linux-x86_64.tar.gz}
bin_dir=$HOME/.local/bin
unit_dir=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user

say() { printf '%s\n' "$*"; }
fail() { printf 'Caw Desktop install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || fail "this installer is for Linux. See https://github.com/$repo#install-windows for Windows."
case $(uname -m) in
  x86_64 | amd64) ;;
  *) fail "Caw Desktop is only built for 64-bit Intel and AMD (x86_64) so far, and this computer is $(uname -m). You can build it yourself: https://github.com/$repo#by-hand" ;;
esac
[ "$(id -u)" != 0 ] || fail "run this as yourself, not as root or with sudo; it installs just for you."
for tool in curl tar; do
  command -v "$tool" >/dev/null 2>&1 || fail "please install $tool first."
done
if command -v dpkg >/dev/null 2>&1 && dpkg -s caw-desktop >/dev/null 2>&1; then
  fail "Caw Desktop is already installed from the .deb package. To upgrade, install the new .deb instead."
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
say "Downloading Caw Desktop…"
curl -fsSL "$url" -o "$tmp/caw-desktop.tar.gz" || fail "couldn't download $url"
tar -xzf "$tmp/caw-desktop.tar.gz" -C "$tmp" || fail "the download isn't a valid tar.gz"
[ -f "$tmp/caw-desktop" ] || fail "caw-desktop is missing from the download"

mkdir -p "$bin_dir" "$unit_dir"
new=$bin_dir/.caw-desktop.new
cp "$tmp/caw-desktop" "$new"
chmod 755 "$new"
"$new" --version >/dev/null 2>&1 || { rm -f "$new"; fail "the downloaded caw-desktop doesn't run on this computer"; }
# Replace by renaming, which is safe while the old version is running.
mv -f "$new" "$bin_dir/caw-desktop"
say "Installed $("$bin_dir/caw-desktop" --version) in $bin_dir"

cat >"$unit_dir/caw-desktop.service" <<'UNIT'
[Unit]
Description=Caw Desktop
After=graphical-session.target
PartOf=graphical-session.target

[Service]
ExecStart=%h/.local/bin/caw-desktop serve
Restart=on-failure

[Install]
WantedBy=graphical-session.target
UNIT

wlroots=${HYPRLAND_INSTANCE_SIGNATURE:-}${SWAYSOCK:-}
case ${XDG_CURRENT_DESKTOP:-} in *[Hh]yprland* | *[Ss]way*) wlroots=yes ;; esac
if [ -n "$wlroots" ] && ! command -v wtype >/dev/null 2>&1; then
  say "Note: on Hyprland and Sway, Caw Desktop also needs wtype. Install it with your package manager (for example: sudo pacman -S wtype)."
fi

if [ -n "${CAW_NO_START:-}" ]; then
  say "Done. Start it with: $bin_dir/caw-desktop serve"
  exit 0
fi

if ! command -v systemctl >/dev/null 2>&1 || ! systemctl --user show-environment >/dev/null 2>&1; then
  say "Couldn't reach systemd for your user, so Caw Desktop won't start by itself."
  say "Start it with: $bin_dir/caw-desktop serve"
  exit 0
fi
systemctl --user daemon-reload
systemctl --user enable caw-desktop.service 2>/dev/null || fail "couldn't enable the caw-desktop service"
say "Starting Caw Desktop. If your desktop asks whether it may control the keyboard and mouse, allow it."
# The receiver makes its control socket once it's ready; wait for a fresh one, so
# pairing below talks to the service instead of starting a receiver of its own.
sock=${XDG_RUNTIME_DIR:-/tmp}/caw-desktop.sock
touch "$tmp/started"
systemctl --user restart caw-desktop.service
i=0
while [ $i -lt 60 ] && ! [ "$sock" -nt "$tmp/started" ]; do
  sleep 1
  i=$((i + 1))
done
say "Caw Desktop is running, and starts by itself when you log in."

case :$PATH: in
  *":$bin_dir:"*) pair=caw-desktop ;;
  *) pair=$bin_dir/caw-desktop ;;
esac
say ""
say "Now pair your phone. (Later, run \`$pair pair\` to pair another one.)"
"$bin_dir/caw-desktop" pair </dev/null
