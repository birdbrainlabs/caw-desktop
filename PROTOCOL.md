# Caw Remote Protocol (v3)

How the Caw phone app sends dictated text (and later, mouse input) to Caw Desktop.

## Overview

- **Transport:** TCP + TLS 1.3, mutual authentication with self-signed certificates.
- **Messages:** newline-delimited JSON (UTF-8), one object per line.
- **Default port:** `47820` (desktop listens; phone connects).
- **Discovery:** mDNS / DNS-SD service type `_caw._tcp.local.`
  TXT records: `id=<desktop_id>`, `name=<desktop name>`, `v=1`.
  On Android use `NsdManager`. The pairing QR also carries host + port, so mDNS is only needed to find the desktop again if its IP changes.

## Identities

Each side has a long-lived EC P-256 key pair and a self-signed X.509 certificate.

- **Fingerprint** = SHA-256 of the DER-encoded certificate (32 bytes).
- **device_id / desktop_id** = first 16 bytes of the fingerprint, lowercase hex.
- Phone: generate the key in Android Keystore (it creates the self-signed cert) and use it as the TLS **client certificate**.
- Both sides accept any certificate at the TLS layer, then check the peer fingerprint themselves (during pairing via the proof below; afterwards against the stored fingerprint).

## Pairing

1. The user runs pairing on the desktop. It shows a **pairing code** and a **QR code** for up to 2 minutes.
   - Code: 16 random characters, Crockford base32 (80 bits), shown as `XXXX-XXXX-XXXX-XXXX`.
     Normalize before use: uppercase, drop `-` and spaces, `O`→`0`, `I`/`L`→`1`.
   - QR content: `caw://pair?h=<ip>&p=<port>&c=<code without dashes>&n=<url-encoded desktop name>`
2. The phone scans the QR (or the user types the host + code) and opens a TLS connection with its client cert.
3. Let `K = UTF-8 bytes of the normalized code`, `D = desktop cert fingerprint`, `P = phone cert fingerprint` (each 32 raw bytes, taken from the TLS session).
4. Phone sends:
   ```json
   {"type":"pair_request","v":1,"device_id":"…","device_name":"Pixel 8","proof":"<base64 HMAC-SHA256(K, \"caw-pair-phone\" || D || P)>"}
   ```
5. Desktop checks the proof (constant-time compare). If it's valid, it stores the phone's fingerprint + name as trusted and replies:
   ```json
   {"type":"pair_ok","desktop_id":"…","desktop_name":"my-laptop","proof":"<base64 HMAC-SHA256(K, \"caw-pair-desktop\" || D || P)>"}
   ```
   Otherwise: `{"type":"pair_error","reason":"bad_code"}` and the connection closes. After 5 failures, pairing mode ends.
6. Phone checks the desktop proof. If it's valid, it stores the desktop fingerprint, name, host and port. Pairing is then done, and the same connection continues as a normal session (skip to `hello`).

The proofs bind the code to both certificates, so someone else on the Wi-Fi can't pair or sit in the middle without the code.

## Session

Every connection after pairing: TLS with both fingerprints checked against the stored values. Unknown phone → desktop closes the connection.

| Direction | Message | Meaning |
|---|---|---|
| phone → desktop | `{"type":"hello","v":1,"device_id":"…"}` | first message |
| desktop → phone | `{"type":"hello_ok","desktop_name":"…"}` | ready |
| phone → desktop | `{"type":"text","id":1,"text":"Hello world. "}` | type this text exactly as given |
| phone → desktop | `{"type":"key","id":2,"key":"backspace","count":1}` | special key, `count` times |
| desktop → phone | `{"type":"ack","id":1}` | message `id` was handled |
| desktop → phone | `{"type":"error","id":1,"reason":"…"}` | message `id` failed |
| either | `{"type":"ping"}` / `{"type":"pong"}` | keepalive; phone pings every 15 s, either side drops the connection after 45 s of silence |

`key` values: `backspace`, `delete`, `enter`, `tab`, `escape`, `left`, `right`, `up`, `down`, `home`, `end`.

The phone is responsible for spacing and corrections (word-fixes) before sending. The desktop types `text` verbatim into the focused window.

Unknown `type` → desktop replies `error` with `reason:"unknown_type"` and keeps the connection open (forward compatibility).

## v2: touchpad

Caw Desktop speaks v2 and still accepts v1 phones (`hello` with `v:1` or `v:2`).

| Direction | Message | Meaning |
|---|---|---|
| phone → desktop | `{"type":"pointer_move","dx":3.5,"dy":-1.0}` | move the mouse; finger movement in **dp** (px ÷ screen density) |
| phone → desktop | `{"type":"click","button":"left"}` | click; `button` is `left`, `right` or `middle` |
| phone → desktop | `{"type":"click","button":"left","action":"down"}` | press and hold (start a drag); `"up"` releases |
| phone → desktop | `{"type":"scroll","dx":0,"dy":12.0}` | scroll; positive `dy` scrolls down, positive `dx` right, in dp |

- `pointer_move` and `scroll` have no `id` and get no `ack` (they are frequent). `click` may carry an `id` to get an `ack`.
- If the computer can't move the mouse it replies `{"type":"error","reason":"pointer_unavailable"}` once per connection (and for every `click` that has an `id`).
- The desktop applies touchpad acceleration (faster swipes move further), so the phone sends raw finger deltas. Tuning on the desktop: `CAW_POINTER_SPEED` (default 1.5) and `CAW_POINTER_ACCEL` (default 0.05).
- Phone gestures, suggested: one-finger drag → `pointer_move`; tap → left `click`; two-finger tap → right `click`; two-finger drag → `scroll` (for natural scrolling send the negated finger movement); long-press then drag → `click` `down`, moves, `click` `up`.
- Send moves at most every ~8–16 ms, adding up the deltas in between, so a fast swipe doesn't flood the connection.

## v3: settings

The phone can view and change the computer's settings. Desktop v3 still accepts v1/v2 phones.

| Direction | Message | Meaning |
|---|---|---|
| phone → desktop | `{"type":"settings_get","id":7}` | ask for the current settings |
| desktop → phone | `{"type":"settings","id":7,"desktop_name":"my-laptop","desktop_version":"0.1.0","word_fixes":{"car app":"Caw app"},"pointer_speed":1.5,"pointer_accel":0.05}` | the current settings |
| phone → desktop | `{"type":"settings_set","id":8,"pointer_speed":2.0}` | change settings; every field except `id` is optional |
| desktop → phone | `{"type":"ack","id":8}` or `{"type":"error","id":8,"reason":"invalid_settings: …"}` | saved, or refused (nothing changes) |

- `word_fixes` in `settings_set` **replaces the whole list**: send the full edited map. Keys are matched as whole phrases, case-insensitively, and are stored lowercase.
- Limits: `pointer_speed` 0.1–10, `pointer_accel` 0–1, at most 500 word fixes, each phrase and replacement up to 100 characters.
- Changes apply immediately and are saved on the computer, so they follow the computer, not the phone.

## Resends

Caw Desktop remembers handled `text` and `key` messages (per phone and session, by id and content) for 10 minutes. A resend after a reconnect is acknowledged without being typed again. If typing failed, the message is not remembered, so a retry still works.

The phone should send a `session` in `hello`: a random string made once per app start and kept across reconnects, e.g. `{"type":"hello","v":3,"device_id":"…","session":"9f2c41d07ab3e655"}`. When the app restarts and its ids start again at 1, the new session keeps new messages from being mistaken for resends. Without a `session`, all connections from that phone count as one session.

## Word fixes on the desktop

Caw Desktop corrects whole phrases before typing, from `~/.config/caw-desktop/word-fixes.json` (`caw-desktop fixes` to list or edit). The phone may still do its own fixes first.
