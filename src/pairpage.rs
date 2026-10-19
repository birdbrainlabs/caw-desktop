//! The pairing page `caw-desktop pair --window` opens in the web browser, so
//! phones can be paired from the apps menu without a terminal. The page is
//! self-contained: the QR code is inline SVG and its policy forbids loading
//! anything, so nothing on it leaves the computer.

use crate::pairing;
use crate::proto::PairUri;
use std::fmt::Write;

/// How to get a new pairing code, on this system.
const AGAIN: &str = if cfg!(target_os = "macos") {
    "Choose <b>Pair phone…</b> from the Caw Desktop icon in the menu bar"
} else {
    "Open <b>Caw Desktop</b> from the apps menu"
};

/// The QR code for `text` as an SVG image, black on white with a quiet zone.
pub fn qr_svg(text: &str) -> Option<String> {
    let qr = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = qr.width();
    let size = width + 8;
    let mut path = String::new();
    for (i, color) in qr.to_colors().into_iter().enumerate() {
        if color == qrcode::Color::Dark {
            let _ = write!(path, "M{} {}h1v1h-1z", i % width + 4, i / width + 4);
        }
    }
    Some(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" shape-rendering="crispEdges" role="img" aria-label="Pairing QR code"><rect width="{size}" height="{size}" fill="#fff"/><path fill="#000" d="{path}"/></svg>"##
    ))
}

/// The page showing a pairing code. `expires_ms` (Unix time) drives the countdown.
pub fn pairing(code: &str, uri: &str, expires_ms: u64) -> String {
    let address = PairUri::parse(uri).map(|u| format!("{}:{}", u.host, u.port)).unwrap_or_default();
    let body = format!(
        r#"<p class="ok">✓ Caw Desktop is running.</p>
<div id="pair">
<p>On your phone, open Caw → <b>Send to computer</b> → <b>Pair</b>, and scan this code:</p>
<div class="qr">{qr}</div>
<p>Or type this code in Caw:</p>
<p class="code">{code}</p>
<p class="small">Computer address: {address}</p>
<p>The code works for 2 minutes (<span id="left">2:00</span> left) and only once.
When Caw says it's paired, you can close this page.</p>
</div>
<div id="expired" hidden>
<p>This code has expired. {AGAIN} again to get a new one.</p>
</div>
<script>
const end = {expires_ms}, left = document.getElementById("left");
function tick() {{
  const s = Math.max(0, Math.round((end - Date.now()) / 1000));
  left.textContent = Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
  if (s > 0) return setTimeout(tick, 1000);
  document.getElementById("pair").hidden = true;
  document.getElementById("expired").hidden = false;
}}
tick();
</script>"#,
        qr = qr_svg(uri).unwrap_or_default(),
        code = escape(&pairing::display(code)),
        address = escape(&address),
    );
    page("Pair your phone", &body)
}

/// The page left behind once pairing has ended; it holds no code.
pub fn outcome(outcome: &str) -> String {
    let text = if let Some(phone) = outcome.strip_prefix("paired with ") {
        format!("✓ Paired with {}. You can close this page.", escape(phone))
    } else {
        format!("Pairing ended: {}. {AGAIN} to try again.", escape(outcome))
    };
    page("Pair your phone", &format!("<p>{text}</p>"))
}

/// A page explaining why pairing couldn't start.
pub fn error(why: &str) -> String {
    let body = format!(
        "<p>Caw Desktop couldn't start pairing:</p><p class=\"small\">{}</p><p>{AGAIN} to try again.</p>",
        escape(why)
    );
    page("Pair your phone", &body)
}

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
<meta name="referrer" content="no-referrer">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} – Caw Desktop</title>
<style>
:root {{ color-scheme: light dark; --bg: #f6f6f4; --fg: #1d1d1b; --muted: #5f5f5a; --ok: #1d7a3a; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg: #1c1c1a; --fg: #ecece8; --muted: #a8a8a2; --ok: #6fcf8a; }} }}
body {{ margin: 0; background: var(--bg); color: var(--fg); font: 17px/1.5 system-ui, sans-serif; }}
main {{ max-width: 34rem; margin: 0 auto; padding: 2rem 1rem; }}
h1 {{ font-size: 1.6rem; margin: 0 0 .5rem; }}
.ok {{ color: var(--ok); font-weight: 600; }}
.qr svg {{ display: block; width: min(20rem, 100%); height: auto; margin: 1rem 0; border-radius: 6px; }}
.code {{ font: 600 1.6rem/1.2 ui-monospace, monospace; letter-spacing: .05em; }}
.small {{ color: var(--muted); font-size: .9rem; }}
</style>
</head>
<body>
<main>
<h1>{title}</h1>
{body}
</main>
</body>
</html>
"#
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_page_has_qr_and_code() {
        let uri = PairUri { host: "192.0.2.7".into(), port: 47820, code: "ABCDEFGHJKMNPQRS".into(), name: "<pc>".into() }.to_uri();
        let html = pairing("ABCDEFGHJKMNPQRS", &uri, 0);
        assert!(html.contains("<svg") && html.contains("<path fill=\"#000\" d=\"M"));
        assert!(html.contains("ABCD-EFGH-JKMN-PQRS"));
        assert!(html.contains("192.0.2.7:47820"));
        assert!(html.contains("default-src 'none'"));
    }

    #[test]
    fn outcome_page_has_no_code_and_escapes() {
        let html = outcome("paired with <Pixel>");
        assert!(html.contains("Paired with &lt;Pixel&gt;"));
        assert!(!html.contains("<svg"));
    }
}
