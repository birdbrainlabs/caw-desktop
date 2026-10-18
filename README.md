# Caw Desktop

Use your phone as a microphone, keyboard and touchpad for your computer.

You dictate into the Caw phone app; Caw Desktop types the text into whatever
window is focused on your computer. The phone can also move the mouse, click,
scroll, and send Backspace or Enter. Speech recognition runs entirely on the
phone, and the phone and computer talk only over your local network.

Caw Desktop is part of the Caw family, with
[Caw for Mac](https://github.com/birdbrainlabs/caw) and the Caw Android app.

> **Status: early.** Used daily on Hyprland. Also works on GNOME, KDE Plasma,
> X11 desktops (Cinnamon, XFCE, MATE) and Windows 10 and 11; those are tested
> automatically but not yet by people, so please report how it goes.

## How it works

1. Caw Desktop runs in the background on your computer.
2. You pair your phone once by scanning a QR code (or typing a code).
3. From then on the phone connects by itself whenever both are on the same Wi-Fi.

The connection is encrypted, and only phones you have paired can connect.
See [Security](#security) and [PROTOCOL.md](PROTOCOL.md).

## Install (Windows)

1. Download [caw-desktop-windows-x86_64.zip](https://github.com/birdbrainlabs/caw-desktop/releases/latest/download/caw-desktop-windows-x86_64.zip)
   from the [latest release](https://github.com/birdbrainlabs/caw-desktop/releases/latest)
   and unzip it into a folder you'll keep, e.g. `Documents\Caw Desktop`.
2. Double-click `caw-desktop.exe`. Windows may say it "protected your PC"
   because the app isn't signed yet: click **More info → Run anyway**.
3. When Windows Firewall asks, allow access on **private networks**, so the
   phone can reach it. A console window stays open while Caw Desktop runs.
4. Double-click `Pair phone.cmd` and scan the QR code with Caw.

To start it automatically, press **Win+R**, type `shell:startup`, and put a
shortcut to `caw-desktop.exe` in the folder that opens.

Windows doesn't let normal apps type into apps running as administrator, so
Caw can't type into those unless Caw Desktop is also run as administrator.

## Install (Linux)

### Ubuntu, Debian and Linux Mint

1. Download [caw-desktop-linux-x86_64.deb](https://github.com/birdbrainlabs/caw-desktop/releases/latest/download/caw-desktop-linux-x86_64.deb)
   from the [latest release](https://github.com/birdbrainlabs/caw-desktop/releases/latest).
2. Double-click it. It opens in the Software app; click **Install**.
   (Or in a terminal: `sudo apt install ~/Downloads/caw-desktop-linux-x86_64.deb`.)
3. Open **Caw Desktop** from the apps menu. It shows a QR code in your web
   browser: in Caw, tap **Send to computer → Pair** and scan it.

Caw Desktop now starts by itself whenever you log in. To pair another phone,
open **Caw Desktop** from the apps menu again. To uninstall, remove it in the
Software app or with `sudo apt remove caw-desktop`.

### Other distributions

One command, no sudo:

```sh
curl -fsSL https://raw.githubusercontent.com/birdbrainlabs/caw-desktop/master/install.sh | sh
```

It puts `caw-desktop` in `~/.local/bin`, starts it now and at every login
with a systemd user service, and shows the QR code to scan. Run it again to
upgrade.

### By hand

Download [caw-desktop-linux-x86_64.tar.gz](https://github.com/birdbrainlabs/caw-desktop/releases/latest/download/caw-desktop-linux-x86_64.tar.gz)
and put `caw-desktop` in `~/.local/bin/`, or build it with Rust (`cargo`):

```sh
git clone https://github.com/birdbrainlabs/caw-desktop
cd caw-desktop
cargo build --release
install -m755 target/release/caw-desktop ~/.local/bin/
```

Then start it at login with a systemd user service in
`~/.config/systemd/user/caw-desktop.service`, enable it with
`systemctl --user enable --now caw-desktop`, and run `caw-desktop pair`:

```ini
[Unit]
Description=Caw Desktop
After=graphical-session.target
PartOf=graphical-session.target

[Service]
ExecStart=%h/.local/bin/caw-desktop serve
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

### Your desktop

Caw Desktop picks the right way to type and move the mouse on your desktop:

- **GNOME and KDE Plasma (Wayland):** the first time it starts, the desktop
  asks whether Caw Desktop may control your keyboard and mouse. Allow it; the
  answer is remembered.
- **X11 desktops** (Linux Mint Cinnamon, XFCE, MATE): nothing to set up.
- **Hyprland and Sway:** install `wtype`.

Characters your keyboard layout doesn't have (like "é" on a US layout) are
typed on X11, Hyprland and Sway, but may be skipped on GNOME and KDE.

If you use a firewall, allow the phone in:

```sh
sudo ufw allow 47820/tcp   # Caw Desktop
sudo ufw allow 5353/udp    # finding the computer on the network (mDNS)
```

## Use

```sh
caw-desktop pair       # show a QR code and pairing code for the phone
caw-desktop pair --window   # the same, in the web browser (the apps menu entry)
caw-desktop devices    # list paired phones
caw-desktop unpair ID  # forget a phone
caw-desktop fixes      # list word fixes
caw-desktop fixes "car app" "Caw app"   # add a word fix
caw-desktop settings   # show mouse speed settings
caw-desktop shortcuts  # list shortcut buttons for the phone
caw-desktop shortcuts add Terminal super+enter   # add a shortcut
caw-desktop shortcuts remove Terminal            # remove it
```

**Word fixes** correct phrases the speech model often mishears before they are
typed. They match whole phrases only, so adding `"car app" → "Caw app"` leaves
the word "car" alone everywhere else. The phone app can also edit them.

**Shortcuts** are buttons on the phone that press a key combo on the computer,
such as `super+enter` (Super is the Windows key) or `ctrl+shift+t`. Keys are
named as in [PROTOCOL.md](PROTOCOL.md#v4-shortcuts): letters, digits, `f1`–`f12`,
`enter`, `space`, `tab`, `escape`, arrows, `page_up`, `minus` and so on. Caw just
presses the keys; what they do is up to your desktop. There are none until you
add some, here or in the phone app's settings; the phone picks up changes made
here the next time it connects.

## Security

- Pairing uses a one-time 16-character code (shown as a QR code) that expires
  after 2 minutes. The code is bound to both devices' certificates, so another
  device on the same network can't pair or intercept the connection without it.
- The pairing page the apps menu opens is a local file only you can read. It
  loads nothing from the internet, and the code is removed from it once
  pairing ends.
- After pairing, both sides pin each other's certificate (mutual TLS 1.3).
  Unpaired devices are disconnected before they can send anything.
- Caw Desktop never sends your text anywhere; it only types what the phone sends.

Found a security problem? Please open a GitHub issue without details and ask
for a private contact, rather than posting the details publicly.

## Development

```sh
cargo test
cargo clippy --all-targets
```

`caw-fake-phone` speaks the phone side of the protocol, for testing without a
phone and as a reference for phone implementations:

```sh
caw-fake-phone pair 'caw://pair?h=<ip>&p=47820&c=<code>&n=x'
caw-fake-phone send 'Hello world. '
caw-fake-phone key backspace 3
caw-fake-phone combo ctrl+shift+t
caw-fake-phone move 200 100
caw-fake-phone click left
caw-fake-phone scroll 30
caw-fake-phone settings
caw-fake-phone set-speed 2
caw-fake-phone set-shortcuts '[{"name":"Terminal","key":"enter","mods":["super"]}]'
```

Run the receiver with `CAW_DRY_RUN=1` to log what it would type instead of
typing, and `CAW_PORT=<port>` to use another port. `CAW_NO_OPEN=1` makes
`pair --window` write its page without opening the browser.

`cargo deb` builds the .deb (the release uses `cargo zigbuild` first, see
[the workflow](.github/workflows/build.yml)).

## License

- Code: MIT, see [LICENSE](LICENSE).
- The names Caw and Crumb: all rights reserved, see [ART-LICENSE.md](ART-LICENSE.md).
