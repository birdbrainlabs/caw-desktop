//! A stand-in for the Caw phone app, for testing Caw Desktop and as a
//! reference implementation of the phone side of PROTOCOL.md.
//!
//! Commands are listed in README.md (run it without arguments for a summary).

use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use caw_desktop::identity::{fingerprint, hex, short_id, Identity};
use caw_desktop::pairing::{self, LABEL_DESKTOP, LABEL_PHONE};
use caw_desktop::proto::{Msg, PairUri, Shortcut, MAX_LINE, VERSION};
use caw_desktop::tlsconf;
use futures::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::{client::TlsStream, TlsConnector};
use tokio_util::codec::{Framed, LinesCodec};

type Conn = Framed<TlsStream<TcpStream>, LinesCodec>;

/// One session per run, like one per app start on the phone.
fn session_tag() -> String {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION.get_or_init(|| format!("{:016x}", rand::random::<u64>())).clone()
}

/// A fresh message id per run, like a phone's increasing counter.
fn next_id() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1)
}

#[derive(Serialize, Deserialize)]
struct Desktop {
    host: String,
    port: u16,
    name: String,
    fingerprint: String,
}

fn dir() -> Result<PathBuf> {
    Ok(dirs::config_dir().context("no config directory")?.join("caw-fake-phone"))
}

async fn connect(identity: &Identity, host: &str, port: u16) -> Result<(Conn, [u8; 32])> {
    let tcp = TcpStream::connect((host, port)).await.with_context(|| format!("connecting to {host}:{port}"))?;
    let tls = TlsConnector::from(tlsconf::client_config(identity)?)
        .connect(ServerName::try_from("caw-desktop")?, tcp)
        .await
        .context("TLS handshake")?;
    let desktop_fp = {
        let (_, conn) = tls.get_ref();
        fingerprint(conn.peer_certificates().and_then(|c| c.first()).context("desktop sent no certificate")?.as_ref())
    };
    Ok((Framed::new(tls, LinesCodec::new_with_max_length(MAX_LINE)), desktop_fp))
}

async fn send(io: &mut Conn, msg: &Msg) -> Result<()> {
    io.send(serde_json::to_string(msg)?).await?;
    Ok(())
}

async fn recv(io: &mut Conn) -> Result<Msg> {
    let line = timeout(Duration::from_secs(15), io.next()).await.context("no reply")?.context("connection closed")??;
    Ok(serde_json::from_str(&line)?)
}

async fn hello(io: &mut Conn, identity: &Identity) -> Result<()> {
    let device_id = short_id(&identity.fingerprint());
    send(io, &Msg::Hello { v: VERSION, device_id, session: Some(session_tag()) }).await?;
    match recv(io).await? {
        Msg::HelloOk { desktop_name } => {
            println!("connected to {desktop_name}");
            Ok(())
        }
        other => bail!("hello refused: {other:?}"),
    }
}

/// Connect to the stored desktop, checking its pinned fingerprint.
async fn session(identity: &Identity) -> Result<Conn> {
    let desktop: Desktop = serde_json::from_slice(&std::fs::read(dir()?.join("desktop.json")).context("not paired yet; run `caw-fake-phone pair <link>`")?)?;
    let (mut io, fp) = connect(identity, &desktop.host, desktop.port).await?;
    ensure!(hex(&fp) == desktop.fingerprint, "desktop certificate changed; refusing to connect (re-pair if this is expected)");
    hello(&mut io, identity).await?;
    Ok(io)
}

/// Round-trips a ping so fire-and-forget messages are handled before hanging up.
async fn finish(io: &mut Conn) -> Result<()> {
    send(io, &Msg::Ping).await?;
    loop {
        match recv(io).await? {
            Msg::Pong => break,
            Msg::Error { reason, .. } => bail!("desktop reported {reason}"),
            _ => {}
        }
    }
    SinkExt::<String>::close(io).await?;
    println!("sent");
    Ok(())
}

async fn expect_ack(io: &mut Conn, id: u64) -> Result<()> {
    match recv(io).await? {
        Msg::Ack { id: got } if got == id => Ok(()),
        other => bail!("desktop replied {other:?}"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let identity = Identity::load_or_create(&dir()?, "caw-fake-phone")?;

    match args.first().map(String::as_str) {
        Some("pair") => {
            let uri = PairUri::parse(args.get(1).context("usage: caw-fake-phone pair <caw://pair link>")?)?;
            let (mut io, desktop_fp) = connect(&identity, &uri.host, uri.port).await?;
            let phone_fp = identity.fingerprint();
            let proof = pairing::proof(&uri.code, LABEL_PHONE, &desktop_fp, &phone_fp);
            send(&mut io, &Msg::PairRequest { v: VERSION, device_id: short_id(&phone_fp), device_name: "Fake phone".into(), proof: B64.encode(proof) }).await?;
            match recv(&mut io).await? {
                Msg::PairOk { desktop_name, proof, .. } => {
                    let proof = B64.decode(proof)?;
                    ensure!(pairing::verify(&uri.code, LABEL_DESKTOP, &desktop_fp, &phone_fp, &proof), "desktop proof is wrong; someone may be in the middle");
                    let desktop = Desktop { host: uri.host, port: uri.port, name: desktop_name, fingerprint: hex(&desktop_fp) };
                    std::fs::write(dir()?.join("desktop.json"), serde_json::to_vec_pretty(&desktop)?)?;
                    println!("paired with {}", desktop.name);
                    hello(&mut io, &identity).await
                }
                other => bail!("pairing refused: {other:?}"),
            }
        }
        Some("send") => {
            let text = args.get(1).context("usage: caw-fake-phone send <text>")?;
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::Text { id, text: text.clone() }).await?;
            expect_ack(&mut io, id).await?;
            if std::env::var_os("CAW_RESEND").is_some() {
                // Simulate Caw resending after a reconnect: same id, same text.
                let mut io2 = session(&identity).await?;
                send(&mut io2, &Msg::Text { id, text: text.clone() }).await?;
                expect_ack(&mut io2, id).await?;
                println!("resent id {id}");
            }
            SinkExt::<String>::close(&mut io).await?;
            println!("sent");
            Ok(())
        }
        Some("key") => {
            let key = args.get(1).context("usage: caw-fake-phone key <name> [count]")?;
            let count = args.get(2).map(|c| c.parse()).transpose()?.unwrap_or(1);
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::Key { id, key: key.clone(), mods: vec![], count }).await?;
            expect_ack(&mut io, id).await?;
            SinkExt::<String>::close(&mut io).await?;
            println!("sent");
            Ok(())
        }
        Some("combo") => {
            // caw-fake-phone combo super+enter  (v4: a key with modifiers)
            let combo = args.get(1).context("usage: caw-fake-phone combo <combo, e.g. ctrl+shift+t> [count]")?;
            let count = args.get(2).map(|c| c.parse()).transpose()?.unwrap_or(1);
            let (key, mods) = Shortcut::parse_combo(combo);
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::Key { id, key, mods, count }).await?;
            expect_ack(&mut io, id).await?;
            SinkExt::<String>::close(&mut io).await?;
            println!("sent");
            Ok(())
        }
        Some("move") => {
            let n = |i: usize| -> Result<f64> { Ok(args.get(i).context("usage: caw-fake-phone move <dx> <dy>")?.parse()?) };
            let (dx, dy) = (n(1)?, n(2)?);
            let mut io = session(&identity).await?;
            // Like a finger swipe: many small moves.
            for _ in 0..10 {
                send(&mut io, &Msg::PointerMove { dx: dx / 10.0, dy: dy / 10.0 }).await?;
                tokio::time::sleep(Duration::from_millis(16)).await;
            }
            finish(&mut io).await
        }
        Some("click") => {
            let button = args.get(1).cloned().unwrap_or_else(|| "left".into());
            let mut io = session(&identity).await?;
            send(&mut io, &Msg::Click { id: Some(1), button, action: "click".into() }).await?;
            expect_ack(&mut io, 1).await?;
            finish(&mut io).await
        }
        Some("scroll") => {
            let dy: f64 = args.get(1).context("usage: caw-fake-phone scroll <dy>")?.parse()?;
            let mut io = session(&identity).await?;
            send(&mut io, &Msg::Scroll { dx: 0.0, dy }).await?;
            finish(&mut io).await
        }
        Some("settings") => {
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::SettingsGet { id }).await?;
            match recv(&mut io).await? {
                Msg::Settings { word_fixes, pointer_speed, pointer_accel, desktop_version, shortcuts, .. } => {
                    println!("desktop version {desktop_version}, pointer speed {pointer_speed}, accel {pointer_accel}");
                    for (heard, fixed) in word_fixes {
                        println!("  {heard:?} -> {fixed:?}");
                    }
                    for s in shortcuts {
                        println!("  shortcut {:?} = {}", s.name, s.combo());
                    }
                }
                other => bail!("unexpected reply {other:?}"),
            }
            finish(&mut io).await
        }
        Some("set-speed") => {
            let speed: f64 = args.get(1).context("usage: caw-fake-phone set-speed <speed>")?.parse()?;
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::SettingsSet { id, word_fixes: None, pointer_speed: Some(speed), pointer_accel: None, shortcuts: None }).await?;
            expect_ack(&mut io, id).await?;
            finish(&mut io).await
        }
        Some("set-fixes") => {
            // caw-fake-phone set-fixes '{"heard":"fixed",...}'  (replaces the whole list)
            let json = args.get(1).context("usage: caw-fake-phone set-fixes '<json object>'")?;
            let word_fixes = serde_json::from_str(json)?;
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::SettingsSet { id, word_fixes: Some(word_fixes), pointer_speed: None, pointer_accel: None, shortcuts: None }).await?;
            expect_ack(&mut io, id).await?;
            finish(&mut io).await
        }
        Some("set-shortcuts") => {
            // caw-fake-phone set-shortcuts '[{"name":"Terminal","key":"enter","mods":["super"]}]'  (replaces the whole list)
            let json = args.get(1).context("usage: caw-fake-phone set-shortcuts '<json array>'")?;
            let shortcuts = serde_json::from_str(json)?;
            let mut io = session(&identity).await?;
            let id = next_id();
            send(&mut io, &Msg::SettingsSet { id, word_fixes: None, pointer_speed: None, pointer_accel: None, shortcuts: Some(shortcuts) }).await?;
            expect_ack(&mut io, id).await?;
            finish(&mut io).await
        }
        _ => bail!("usage: caw-fake-phone pair <link> | send <text> | key <name> [count] | combo <combo> [count] | move <dx> <dy> | click [left|right|middle] | scroll <dy> | settings | set-speed <n> | set-fixes '<json>' | set-shortcuts '<json>'"),
    }
}
