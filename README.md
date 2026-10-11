# Caw Desktop

Use your phone as a microphone, keyboard and touchpad for your computer.

You dictate into the Caw phone app; Caw Desktop types the text into whatever
window is focused on your computer. The phone can also move the mouse, click,
scroll, and send Backspace or Enter. Speech recognition runs entirely on the
phone, and the phone and computer talk only over your local network.

Caw Desktop is part of the Caw family, with
[Caw for Mac](https://github.com/birdbrainlabs/caw) and the Caw Android app.

> **Status: early.** Works on Linux desktops based on wlroots (Hyprland, Sway).
> Support for other Linux desktops (GNOME, KDE Plasma, Cinnamon, X11) and
> Windows is in progress.

## How it works

1. Caw Desktop runs in the background on your computer.
2. You pair your phone once by scanning a QR code (or typing a code).
3. From then on the phone connects by itself whenever both are on the same Wi-Fi.

The connection is encrypted, and only phones you have paired can connect.
See [Security](#security) and [PROTOCOL.md](PROTOCOL.md).

## Install (Linux)

You need Rust (`cargo`) and `wtype`.

```sh
git clone https://github.com/birdbrainlabs/caw-desktop
cd caw-desktop
cargo build --release
install -m755 target/release/caw-desktop ~/.local/bin/
```

Start it automatically when you log in, with a systemd user service at
`~/.config/systemd/user/caw-desktop.service`:

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

```sh
systemctl --user enable --now caw-desktop
```

If you use a firewall, allow the phone in:

```sh
sudo ufw allow 47820/tcp   # Caw Desktop
sudo ufw allow 5353/udp    # finding the computer on the network (mDNS)
```

## Use

```sh
caw-desktop pair       # show a QR code and pairing code for the phone
caw-desktop devices    # list paired phones
caw-desktop unpair ID  # forget a phone
caw-desktop fixes      # list word fixes
caw-desktop fixes "car app" "Caw app"   # add a word fix
caw-desktop settings   # show mouse speed settings
```

**Word fixes** correct phrases the speech model often mishears before they are
typed. They match whole phrases only, so adding `"car app" → "Caw app"` leaves
the word "car" alone everywhere else. The phone app can also edit them.

## Security

- Pairing uses a one-time 16-character code (shown as a QR code) that expires
  after 2 minutes. The code is bound to both devices' certificates, so another
  device on the same network can't pair or intercept the connection without it.
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
caw-fake-phone move 200 100
caw-fake-phone click left
caw-fake-phone scroll 30
caw-fake-phone settings
caw-fake-phone set-speed 2
```

Run the receiver with `CAW_DRY_RUN=1` to log what it would type instead of
typing, and `CAW_PORT=<port>` to use another port.

## License

- Code: MIT, see [LICENSE](LICENSE).
- The names Caw and Crumb: all rights reserved, see [ART-LICENSE.md](ART-LICENSE.md).
