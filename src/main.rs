//! Caw Desktop: receives dictated text from the Caw phone app and types it
//! into the focused window. Protocol: PROTOCOL.md.

#[cfg(all(target_os = "linux", feature = "app"))]
mod app;
mod cgevent;
#[cfg(all(target_os = "linux", feature = "dictation"))]
mod dictate;
mod fixes;
#[cfg_attr(not(all(target_os = "linux", feature = "dictation")), allow(dead_code))]
mod fsutil;
mod headless;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macapp;
mod pointer;
mod sendinput;
mod settings;
mod shortcuts;
mod typer;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use caw_desktop::identity::{fingerprint, hex, short_id, Identity};
use caw_desktop::pairing::{self, LABEL_DESKTOP, LABEL_PHONE};
#[cfg(unix)]
use caw_desktop::pairpage;
use caw_desktop::proto::{Msg, PairUri, Shortcut, DEFAULT_PORT, MAX_LINE, SERVICE_TYPE, VERSION};
use caw_desktop::tlsconf;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::{Framed, LinesCodec};

const PAIR_WINDOW: Duration = Duration::from_secs(120);
const MAX_PAIR_FAILURES: u32 = 5;
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a handled text/key is remembered, to skip resends after a reconnect.
const DEDUP_WINDOW: Duration = Duration::from_secs(600);
const DEDUP_MAX: usize = 256;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Device {
    id: String,
    name: String,
    fingerprint: String,
    paired_at: u64,
}

struct PairWindow {
    code: String,
    expires: Instant,
    failures: u32,
    done: Option<oneshot::Sender<String>>,
}

impl PairWindow {
    fn finish(&mut self, result: String) {
        if let Some(tx) = self.done.take() {
            let _ = tx.send(result);
        }
    }
}

struct State {
    config_dir: PathBuf,
    name: String,
    fp: [u8; 32],
    port: u16,
    pairing: Mutex<Option<PairWindow>>,
    pointer: Option<pointer::Pointer>,
    recent: Mutex<std::collections::VecDeque<(u64, Instant)>>,
    settings: Mutex<settings::Settings>,
    /// Bumped whenever a phone is paired or removed, or phones are turned
    /// off, so connections that may no longer type close at once.
    devices_changed: tokio::sync::watch::Sender<u64>,
    /// "Let phones type on this computer" (the app's switch; always on otherwise).
    /// While off the receiver doesn't listen, except while pairing.
    phones_on: std::sync::atomic::AtomicBool,
    /// The app's pairing window is open: listen even while phones are off.
    pair_window_open: std::sync::atomic::AtomicBool,
    /// Wakes the listener when `receiving()` may have changed.
    gate: tokio::sync::Notify,
}

impl State {
    /// Whether the receiver listens for phones now.
    fn receiving(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        self.phones_on.load(Relaxed) || self.pair_window_open.load(Relaxed) || self.pairing.lock().unwrap().as_ref().is_some_and(|w| Instant::now() < w.expires)
    }

    /// Turns "Let phones type on this computer" on or off.
    #[cfg_attr(not(all(target_os = "linux", feature = "app")), allow(dead_code))]
    fn set_phones(&self, on: bool) {
        self.phones_on.store(on, std::sync::atomic::Ordering::Relaxed);
        self.gate_changed();
    }

    #[cfg_attr(not(all(target_os = "linux", feature = "app")), allow(dead_code))]
    fn set_pair_window(&self, open: bool) {
        self.pair_window_open.store(open, std::sync::atomic::Ordering::Relaxed);
        self.gate_changed();
    }

    fn gate_changed(&self) {
        self.gate.notify_one();
        self.devices_changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// True if this exact message (same phone, session, id and content) was already handled recently.
    /// Caw resends queued messages with their original ids after a reconnect; its ids restart
    /// when the app restarts, which is why the session is part of the key.
    fn already_handled(&self, phone_fp: &[u8; 32], session: &str, msg: &Msg) -> bool {
        let key = dedup_key(phone_fp, session, msg);
        let now = Instant::now();
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|(_, at)| now.duration_since(*at) < DEDUP_WINDOW);
        if recent.iter().any(|(k, _)| *k == key) {
            return true;
        }
        if recent.len() >= DEDUP_MAX {
            recent.pop_front();
        }
        recent.push_back((key, now));
        false
    }

    /// Forget a message whose handling failed, so a retry is not skipped.
    fn forget(&self, phone_fp: &[u8; 32], session: &str, msg: &Msg) {
        let key = dedup_key(phone_fp, session, msg);
        self.recent.lock().unwrap().retain(|(k, _)| *k != key);
    }

    fn fixes_path(&self) -> PathBuf {
        self.config_dir.join("word-fixes.json")
    }

    fn shortcuts_path(&self) -> PathBuf {
        self.config_dir.join("shortcuts.json")
    }

    /// Validates everything first, then saves; nothing changes if any value is bad.
    fn update_settings(&self, word_fixes: Option<fixes::Fixes>, speed: Option<f64>, accel: Option<f64>, shortcuts: Option<Vec<Shortcut>>) -> Result<()> {
        let mut next = *self.settings.lock().unwrap();
        if let Some(v) = speed {
            settings::check_range("pointer_speed", v, settings::SPEED_RANGE)?;
            next.pointer_speed = v;
        }
        if let Some(v) = accel {
            settings::check_range("pointer_accel", v, settings::ACCEL_RANGE)?;
            next.pointer_accel = v;
        }
        let word_fixes = word_fixes.map(|f| f.into_iter().map(|(k, v)| (k.trim().to_lowercase(), v)).collect::<fixes::Fixes>());
        if let Some(f) = &word_fixes {
            settings::check_fixes(f)?;
        }
        let shortcuts = shortcuts.map(shortcuts::check).transpose()?;
        if let Some(f) = &word_fixes {
            fixes::save(&self.fixes_path(), f)?;
        }
        if let Some(list) = &shortcuts {
            shortcuts::save(&self.shortcuts_path(), list)?;
        }
        next.save(&self.config_dir.join("settings.json"))?;
        *self.settings.lock().unwrap() = next;
        Ok(())
    }

    fn devices_path(&self) -> PathBuf {
        self.config_dir.join("devices.json")
    }

    fn is_trusted(&self, fp: &[u8; 32]) -> bool {
        let fp = hex(fp);
        load_devices(&self.devices_path()).iter().any(|d| d.fingerprint == fp)
    }

    fn phone_name(&self, fp: &[u8; 32]) -> String {
        let fp = hex(fp);
        load_devices(&self.devices_path()).into_iter().find(|d| d.fingerprint == fp).map(|d| d.name).unwrap_or_else(|| "phone".into())
    }

    fn trust(&self, fp: &[u8; 32], name: &str) -> Result<()> {
        let path = self.devices_path();
        let fp_hex = hex(fp);
        let mut devices = load_devices(&path);
        devices.retain(|d| d.fingerprint != fp_hex);
        devices.push(Device { id: short_id(fp), name: name.to_string(), fingerprint: fp_hex, paired_at: now() });
        save_devices(&path, &devices)?;
        self.devices_updated();
        Ok(())
    }

    /// Forgets a paired phone; false if no phone has that id.
    fn remove_device(&self, id: &str) -> Result<bool> {
        let path = self.devices_path();
        let mut devices = load_devices(&path);
        let before = devices.len();
        devices.retain(|d| d.id != id);
        if devices.len() == before {
            return Ok(false);
        }
        save_devices(&path, &devices)?;
        self.devices_updated();
        Ok(true)
    }

    fn paired_phones(&self) -> Vec<headless::PairedPhone> {
        load_devices(&self.devices_path()).into_iter().map(|d| headless::PairedPhone { id: d.id, name: d.name, paired_at: d.paired_at }).collect()
    }

    fn devices_updated(&self) {
        self.devices_changed.send_modify(|n| *n = n.wrapping_add(1));
        headless::emit(&headless::Event::Devices { devices: self.paired_phones() });
    }

    /// Closes the open pairing code, if any.
    fn cancel_pairing(&self) {
        if let Some(mut win) = self.pairing.lock().unwrap().take() {
            win.finish("cancelled".into());
        }
        self.gate.notify_one();
    }

    /// Opens a pairing window and returns (code, uri, receiver for the outcome).
    fn start_pairing(&self) -> (String, String, oneshot::Receiver<String>) {
        let code = pairing::new_code();
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.pairing.lock().unwrap();
            if let Some(old) = slot.as_mut() {
                old.finish("replaced by a new pairing code".into());
            }
            *slot = Some(PairWindow { code: code.clone(), expires: Instant::now() + PAIR_WINDOW, failures: 0, done: Some(tx) });
        }
        // While phones are off, the receiver starts listening for this pairing.
        self.gate.notify_one();
        let uri = PairUri { host: local_ip().to_string(), port: self.port, code: code.clone(), name: self.name.clone() };
        (code, uri.to_uri(), rx)
    }

    /// Checks a pair_request. On success stores the phone and returns the desktop proof.
    fn try_pair(&self, phone_fp: &[u8; 32], device_id: &str, device_name: &str, proof_b64: &str) -> Result<Vec<u8>, &'static str> {
        let mut slot = self.pairing.lock().unwrap();
        let Some(win) = slot.as_mut() else { return Err("not_pairing") };
        if Instant::now() > win.expires {
            win.finish("expired".into());
            *slot = None;
            return Err("not_pairing");
        }
        let proof = B64.decode(proof_b64).unwrap_or_default();
        let ok = device_id == short_id(phone_fp) && pairing::verify(&win.code, LABEL_PHONE, &self.fp, phone_fp, &proof);
        if !ok {
            win.failures += 1;
            if win.failures >= MAX_PAIR_FAILURES {
                win.finish("too many wrong codes".into());
                *slot = None;
            }
            return Err("bad_code");
        }
        if self.trust(phone_fp, device_name).is_err() {
            return Err("storage_failed");
        }
        let reply = pairing::proof(&win.code, LABEL_DESKTOP, &self.fp, phone_fp);
        win.finish(format!("paired with {device_name}"));
        *slot = None;
        Ok(reply)
    }
}

/// Names of the phones connected right now, for the menu bar on a Mac.
static CONNECTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A phone listed in CONNECTED (and, headless, reported as connected) while this lives.
struct Connected(headless::Phone);

impl Connected {
    fn new(id: String, name: String) -> Self {
        CONNECTED.lock().unwrap().push(name.clone());
        headless::emit(&headless::Event::Connected { id: id.clone(), name: name.clone() });
        Self(headless::Phone { id, name })
    }
}

impl Drop for Connected {
    fn drop(&mut self) {
        let mut list = CONNECTED.lock().unwrap();
        if let Some(i) = list.iter().position(|n| *n == self.0.name) {
            list.remove(i);
        }
        headless::emit(&headless::Event::Disconnected { id: self.0.id.clone(), name: self.0.name.clone() });
    }
}

fn dedup_key(phone_fp: &[u8; 32], session: &str, msg: &Msg) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    phone_fp.hash(&mut h);
    session.hash(&mut h);
    serde_json::to_string(msg).unwrap_or_default().hash(&mut h);
    h.finish()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load_devices(path: &Path) -> Vec<Device> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_devices(path: &Path, devices: &[Device]) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(devices)?)?;
    Ok(())
}

/// The LAN address other devices can reach (no packets are sent).
fn local_ip() -> IpAddr {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}

fn config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir().context("no config directory")?.join("caw-desktop"))
}

/// The local channel `caw-desktop pair` uses to reach the running receiver:
/// a socket file in the private runtime folder on Linux, a named pipe on Windows.
#[cfg(unix)]
mod control {
    use std::io::Result;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    /// CAW_CONTROL_SOCKET overrides it (for running a second, test receiver).
    fn path() -> PathBuf {
        match std::env::var_os("CAW_CONTROL_SOCKET") {
            Some(p) => p.into(),
            None => dirs::runtime_dir().unwrap_or_else(std::env::temp_dir).join("caw-desktop.sock"),
        }
    }

    pub type Stream = UnixStream;

    pub async fn connect() -> Result<UnixStream> {
        UnixStream::connect(path()).await
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub fn bind() -> Result<Self> {
            let _ = std::fs::remove_file(path());
            UnixListener::bind(path()).map(Self)
        }

        pub async fn accept(&mut self) -> Result<UnixStream> {
            Ok(self.0.accept().await?.0)
        }
    }

    /// Removes the socket file, for a receiver that is quitting.
    #[cfg_attr(not(feature = "app"), allow(dead_code))]
    pub fn remove() {
        let _ = std::fs::remove_file(path());
    }
}

#[cfg(windows)]
mod control {
    use std::io::Result;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};

    fn name() -> String {
        let user = std::env::var("USERNAME").unwrap_or_default();
        format!(r"\\.\pipe\caw-desktop-{user}")
    }

    pub async fn connect() -> Result<NamedPipeClient> {
        ClientOptions::new().open(name())
    }

    pub struct Listener(NamedPipeServer);

    impl Listener {
        pub fn bind() -> Result<Self> {
            ServerOptions::new().first_pipe_instance(true).create(name()).map(Self)
        }

        /// Waits for a client, then opens the next pipe instance for the one after.
        pub async fn accept(&mut self) -> Result<NamedPipeServer> {
            self.0.connect().await?;
            let next = ServerOptions::new().create(name())?;
            Ok(std::mem::replace(&mut self.0, next))
        }
    }
}

/// Listens on IPv6 and IPv4 at once. Phones that find the computer by mDNS may
/// only get its IPv6 address. Windows needs dual-stack asked for explicitly.
fn listen(port: u16) -> Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let dual_stack = || -> std::io::Result<std::net::TcpListener> {
        let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_only_v6(false)?;
        #[cfg(unix)]
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
        socket.listen(128)?;
        Ok(socket.into())
    };
    let listener = match dual_stack() {
        Ok(l) => l,
        Err(_) => std::net::TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("listening on port {port}"))?,
    };
    listener.set_nonblocking(true)?;
    Ok(TcpListener::from_std(listener)?)
}

fn log(msg: impl AsRef<str>) {
    eprintln!("caw-desktop: {}", msg.as_ref());
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = tokio::runtime::Runtime::new()?;
    if args.first().is_some_and(|a| a == "serve") && args.get(1).is_some_and(|a| a == "--headless") {
        headless::enable();
        let code = match runtime.block_on(serve(false)) {
            Ok(()) => 0,
            Err(e) => {
                log(format!("{e:#}"));
                headless::emit(&headless::Event::Stopped { reason: "failed".into(), message: format!("{e:#}") });
                1
            }
        };
        // Don't wait for anything still running (a phone mid-connection, the mDNS thread).
        runtime.shutdown_timeout(Duration::from_secs(1));
        std::process::exit(code);
    }
    if args.first().is_some_and(|a| a == "app") {
        #[cfg(all(target_os = "linux", feature = "app"))]
        return app::run(runtime);
        #[cfg(not(all(target_os = "linux", feature = "app")))]
        bail!("this build of Caw Desktop has no app (it needs Linux and the \"app\" feature)");
    }
    // Caw Desktop.app opened from Finder or at login: the receiver with a menu bar icon.
    #[cfg(target_os = "macos")]
    if args.iter().all(|a| a.starts_with("-psn_")) && macapp::in_bundle() {
        return mac_app(runtime);
    }
    runtime.block_on(cli(args))
}

async fn cli(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        None | Some("serve") => serve(false).await,
        Some("pair") if args.get(1).is_some_and(|a| a == "--window") => pair_window().await,
        Some("pair") => pair().await,
        Some("--version" | "version") => {
            println!("caw-desktop {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("devices") => {
            let devices = load_devices(&config_dir()?.join("devices.json"));
            if devices.is_empty() {
                println!("No paired phones. Run: caw-desktop pair");
            }
            for d in devices {
                println!("{}  {}", d.id, d.name);
            }
            Ok(())
        }
        Some("fixes") => {
            let path = config_dir()?.join("word-fixes.json");
            std::fs::create_dir_all(config_dir()?)?;
            fixes::ensure_file(&path)?;
            let mut list = fixes::load(&path);
            match (args.get(1), args.get(2)) {
                (Some(heard), Some(fixed)) => {
                    list.insert(heard.to_lowercase(), fixed.clone());
                    fixes::save(&path, &list)?;
                    println!("Added: \"{heard}\" → \"{fixed}\"");
                }
                (Some(heard), None) => {
                    if list.remove(&heard.to_lowercase()).is_none() {
                        bail!("no fix for {heard:?}");
                    }
                    fixes::save(&path, &list)?;
                    println!("Removed fix for \"{heard}\"");
                }
                _ => {
                    for (heard, fixed) in &list {
                        println!("  \"{heard}\" → \"{fixed}\"");
                    }
                    println!("File: {}", path.display());
                }
            }
            Ok(())
        }
        Some("shortcuts") => {
            let path = config_dir()?.join("shortcuts.json");
            let mut list = shortcuts::load(&path);
            match (args.get(1).map(String::as_str), args.get(2), args.get(3)) {
                (Some("add"), Some(name), Some(combo)) => {
                    let (key, mods) = Shortcut::parse_combo(combo);
                    list.push(Shortcut { name: name.clone(), key, mods });
                    let list = shortcuts::check(list)?;
                    std::fs::create_dir_all(config_dir()?)?;
                    shortcuts::save(&path, &list)?;
                    let added = list.last().context("empty list")?;
                    println!("Added: \"{}\" = {}", added.name, added.combo());
                }
                (Some("remove"), Some(name), None) => {
                    let before = list.len();
                    list.retain(|s| s.name.to_lowercase() != name.trim().to_lowercase());
                    if list.len() == before {
                        bail!("no shortcut named {name:?}");
                    }
                    shortcuts::save(&path, &list)?;
                    println!("Removed \"{}\"", name.trim());
                }
                (None, ..) => {
                    if list.is_empty() {
                        println!("No shortcuts. Add one: caw-desktop shortcuts add Terminal super+enter");
                    }
                    for s in &list {
                        println!("  \"{}\" = {}", s.name, s.combo());
                    }
                    println!("File: {}", path.display());
                }
                _ => bail!("usage: caw-desktop shortcuts [add <name> <combo> | remove <name>]"),
            }
            Ok(())
        }
        Some("settings") => {
            let s = settings::Settings::load(&config_dir()?.join("settings.json"));
            println!("pointer speed: {}  (phone can change this)", s.pointer_speed);
            println!("pointer acceleration: {}", s.pointer_accel);
            println!("word fixes: caw-desktop fixes");
            println!("shortcuts: caw-desktop shortcuts");
            Ok(())
        }
        #[cfg(all(target_os = "linux", feature = "dictation"))]
        Some("dictate") => match args.get(1).map(String::as_str) {
            None => dictate::run(config_dir()?).await,
            Some("--file") => {
                let Some(file) = args.get(2) else { bail!("usage: caw-desktop dictate --file x.wav [--live] [--type]") };
                let rest = &args[3..];
                dictate::transcribe_file(config_dir()?, Path::new(file), rest.iter().any(|a| a == "--live"), rest.iter().any(|a| a == "--type")).await
            }
            Some(other) => bail!("unknown option {other:?}; usage: caw-desktop dictate [--file x.wav [--live] [--type]]"),
        },
        #[cfg(all(target_os = "linux", feature = "dictation"))]
        Some("talk") => dictate::talk(config_dir()?, &args[1..]).await,
        #[cfg(not(all(target_os = "linux", feature = "dictation")))]
        Some("dictate" | "talk") => bail!("this build of Caw Desktop has no dictation (it needs Linux and the \"dictation\" feature)"),
        Some("unpair") => {
            let Some(id) = args.get(1) else { bail!("usage: caw-desktop unpair <id>") };
            let path = config_dir()?.join("devices.json");
            let mut devices = load_devices(&path);
            let before = devices.len();
            devices.retain(|d| &d.id != id);
            if devices.len() == before {
                bail!("no paired phone with id {id}");
            }
            save_devices(&path, &devices)?;
            println!("Unpaired {id}");
            Ok(())
        }
        _ => {
            #[cfg(all(target_os = "linux", feature = "app"))]
            println!("caw-desktop app        the app: receiver, dictation, tray icon and the floating pill");
            println!("caw-desktop            run the receiver (same as `serve`)");
            println!("caw-desktop serve --headless  the receiver for another app: JSON lines on stdin/stdout");
            println!("caw-desktop pair       show a pairing code and QR code for the phone");
            println!("caw-desktop pair --window  the same, as a page in the web browser");
            println!("caw-desktop devices    list paired phones");
            println!("caw-desktop unpair ID  forget a phone");
            println!("caw-desktop settings  show mouse settings");
            println!("caw-desktop fixes                     list word fixes");
            println!("caw-desktop fixes \"heard\" \"fixed\"     add a word fix");
            println!("caw-desktop fixes \"heard\"             remove a word fix");
            println!("caw-desktop shortcuts                          list shortcut buttons for the phone");
            println!("caw-desktop shortcuts add \"name\" super+enter   add one (keys: see PROTOCOL.md)");
            println!("caw-desktop shortcuts remove \"name\"            remove one");
            #[cfg(all(target_os = "linux", feature = "dictation"))]
            {
                println!("caw-desktop dictate       hold-to-talk dictation on this computer (keeps the speech model loaded)");
                println!("caw-desktop dictate --file x.wav [--live] [--type]  transcribe a WAV file, with timings");
                println!("caw-desktop talk start|stop|toggle  start or stop recording (bind to a key's press and release)");
                println!("caw-desktop talk status   is it recording?");
                println!("caw-desktop talk bindings [key]  the Hyprland lines for the talk key (e.g. ctrl_r, alt_r, fn, F13)");
            }
            Ok(())
        }
    }
}

/// The Mac app: the receiver on background threads, the menu bar icon on this
/// (the main) thread. Opened again while one runs, it shows the pairing page
/// from the running one and quits.
#[cfg(target_os = "macos")]
fn mac_app(runtime: tokio::runtime::Runtime) -> Result<()> {
    if runtime.block_on(control::connect()).is_ok() {
        return runtime.block_on(pair_window());
    }
    let handle = runtime.handle().clone();
    let stopped: Arc<Mutex<Option<String>>> = Arc::default();
    let why = stopped.clone();
    std::thread::spawn(move || {
        if let Err(e) = runtime.block_on(serve(false)) {
            log(format!("{e:#}"));
            *why.lock().unwrap() = Some(format!("{e:#}"));
        }
    });
    macapp::run(macapp::Hooks {
        pair: Box::new(move || {
            handle.spawn(async {
                if let Err(e) = pair_from_menu().await {
                    log(format!("pairing: {e:#}"));
                }
            });
        }),
        status: Box::new(move || {
            if let Some(why) = stopped.lock().unwrap().as_ref() {
                return format!("Not running: {why}");
            }
            let phones = CONNECTED.lock().unwrap().clone();
            if phones.is_empty() {
                "No phone connected".into()
            } else {
                format!("Connected: {}", phones.join(", "))
            }
        }),
        config_dir: config_dir()?,
    })
}

/// Ask a running receiver for a pairing code; start one if none is running.
async fn pair() -> Result<()> {
    match control::connect().await {
        Ok(stream) => {
            let outcome = ask_to_pair(stream, |code, uri| {
                show_pairing(code, uri);
                Ok(())
            })
            .await?;
            println!("{}", outcome_text(&outcome));
            Ok(())
        }
        Err(_) => serve(true).await,
    }
}

/// Asks the running receiver for a pairing code, shows it, and waits for the outcome.
async fn ask_to_pair<S>(stream: S, show: impl FnOnce(&str, &str) -> Result<()>) -> Result<String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite,
{
    let (read, mut write) = tokio::io::split(stream);
    write.write_all(b"pair\n").await?;
    let mut lines = BufReader::new(read).lines();
    let first = lines.next_line().await?.context("receiver closed the connection")?;
    let (code, uri): (String, String) = serde_json::from_str(&first)?;
    show(&code, &uri)?;
    Ok(lines.next_line().await?.unwrap_or_else(|| "receiver stopped".into()))
}

#[cfg(not(unix))]
async fn pair_window() -> Result<()> {
    bail!("`pair --window` is only for Linux and macOS; use `caw-desktop pair`")
}

/// `pair --window`, for the apps menu: starts the receiver if it isn't running,
/// then shows the pairing code as a page in the web browser instead of a terminal.
/// The page is private to this user (mode 0600 in a private folder) and is
/// rewritten without the code once pairing ends. `CAW_NO_OPEN=1` skips the browser.
#[cfg(unix)]
async fn pair_window() -> Result<()> {
    let page = pair_page_path()?;
    let stream = match connect_or_start().await {
        Ok(s) => s,
        Err(e) => {
            show_page(&page, &pairpage::error(&format!("{e:#}")))?;
            return Err(e);
        }
    };
    pair_in_page(&page, stream).await
}

/// "Pair phone…" in the Mac menu bar: the same page, from the receiver running in this app.
#[cfg(target_os = "macos")]
async fn pair_from_menu() -> Result<()> {
    let page = pair_page_path()?;
    let stream = match control::connect().await {
        Ok(s) => s,
        Err(e) => {
            show_page(&page, &pairpage::error(&format!("the receiver isn't running ({e})")))?;
            return Err(e.into());
        }
    };
    pair_in_page(&page, stream).await
}

/// Shows a new pairing code on the page, then what came of it.
#[cfg(unix)]
async fn pair_in_page(page: &Path, stream: control::Stream) -> Result<()> {
    let outcome = ask_to_pair(stream, |code, uri| {
        let expires_ms = now() * 1000 + PAIR_WINDOW.as_millis() as u64;
        show_page(page, &pairpage::pairing(code, uri, expires_ms))
    })
    .await?;
    write_page(page, &pairpage::outcome(&outcome))?;
    println!("{}", outcome_text(&outcome));
    Ok(())
}

/// Connects to the running receiver, first starting one in the background if needed.
#[cfg(unix)]
async fn connect_or_start() -> Result<control::Stream> {
    use std::io::IsTerminal;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    if let Ok(stream) = control::connect().await {
        return Ok(stream);
    }
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("serve").stdin(Stdio::null()).stdout(Stdio::null());
    if std::io::stderr().is_terminal() {
        cmd.stderr(Stdio::null());
    }
    // Its own process group, so it keeps running after this process and any terminal are gone.
    let mut child = cmd.process_group(0).spawn().context("starting the receiver")?;
    // The first start on GNOME and KDE waits until the user allows keyboard and mouse control.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(stream) = control::connect().await {
            return Ok(stream);
        }
        if let Some(status) = child.try_wait()? {
            // It exits at once, successfully, if another receiver started meanwhile.
            return control::connect().await.with_context(|| format!("the receiver stopped ({status})"));
        }
        if Instant::now() > deadline {
            bail!("the receiver didn't start within 3 minutes");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Where the pairing page goes: the private runtime folder, or the browser's own
/// folder in it when the browser is a snap (as Firefox and Chromium are on Ubuntu),
/// since snaps can't read the rest of it.
#[cfg(target_os = "linux")]
fn pair_page_path() -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let runtime = dirs::runtime_dir().context("XDG_RUNTIME_DIR is not set")?;
    let handler = std::process::Command::new("xdg-mime")
        .args(["query", "default", "text/html"])
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // Snaps' menu entries are named `<snap>_<app>.desktop`.
    let snap = handler.split_once('_').map(|(name, _)| name).filter(|name| {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && Path::new("/snap").join(name).is_dir()
    });
    let dir = match snap {
        Some(name) => runtime.join(format!("snap.{name}")),
        None => runtime,
    };
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    Ok(dir.join(format!("caw-desktop-pair-{}.html", std::process::id())))
}

/// On a Mac: a private folder (0700) in this user's temporary folder, one page per pairing.
#[cfg(target_os = "macos")]
fn pair_page_path() -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let dir = std::env::temp_dir().join("caw-desktop");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    if !std::fs::symlink_metadata(&dir)?.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    // Fails unless the folder is this user's.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir.join(format!("caw-desktop-pair-{}-{}.html", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))))
}

/// Writes the page readable only by this user, replacing any earlier one.
#[cfg(unix)]
fn write_page(path: &Path, html: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    // create_new also refuses to follow a symlink left at this path.
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).with_context(|| format!("writing {}", path.display()))?;
    file.write_all(html.as_bytes())?;
    Ok(())
}

#[cfg(unix)]
fn show_page(path: &Path, html: &str) -> Result<()> {
    write_page(path, html)?;
    println!("Pairing page: {}", path.display());
    if std::env::var_os("CAW_NO_OPEN").is_some() {
        return Ok(());
    }
    let opener = if cfg!(target_os = "macos") { "/usr/bin/open" } else { "xdg-open" };
    let mut opener = std::process::Command::new(opener)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .context("opening the web browser (is xdg-open installed?)")?;
    std::thread::spawn(move || opener.wait());
    Ok(())
}

fn outcome_text(outcome: &str) -> String {
    if outcome.starts_with("paired") {
        format!("✓ {outcome}. You can close this window.")
    } else {
        format!("✗ Pairing ended: {outcome}. Run `caw-desktop pair` to try again.")
    }
}

fn show_pairing(code: &str, uri: &str) {
    use qrcode::render::unicode::Dense1x2;
    println!();
    println!("  Scan this with Caw → Send to computer → Pair:");
    println!();
    if let Ok(qr) = qrcode::QrCode::new(uri.as_bytes()) {
        // Inverted colors so it scans correctly from a dark terminal.
        let art = qr.render::<Dense1x2>().dark_color(Dense1x2::Light).light_color(Dense1x2::Dark).quiet_zone(true).build();
        for line in art.lines() {
            println!("  {line}");
        }
    }
    println!();
    println!("  Or type this code in Caw:  {}", pairing::display(code));
    println!("  Computer address:          {}", PairUri::parse(uri).map(|u| format!("{}:{}", u.host, u.port)).unwrap_or_default());
    println!();
    println!("  The code works for 2 minutes. Waiting for the phone…");
}

async fn serve(start_pairing: bool) -> Result<()> {
    serve_with(ServeOptions { start_pairing, phones_on: true, ready: None }).await
}

struct ServeOptions {
    /// Open a pairing code at once and print it (`caw-desktop pair` with no receiver running).
    start_pairing: bool,
    /// "Let phones type on this computer"; off, the receiver listens only while pairing.
    phones_on: bool,
    /// Gets the receiver's state once it is set up (the app).
    ready: Option<oneshot::Sender<Arc<State>>>,
}

async fn serve_with(opts: ServeOptions) -> Result<()> {
    let ServeOptions { start_pairing, phones_on, ready } = opts;
    // Started twice (say by both a login item and a systemd unit), the second one bows out
    // instead of fighting over the port and asking again for keyboard and mouse control.
    if control::connect().await.is_ok() {
        log("already running");
        if headless::on() {
            headless::emit(&headless::Event::Stopped {
                reason: "already_running".into(),
                message: "Caw Desktop is already running on this computer".into(),
            });
            std::process::exit(headless::EXIT_IN_USE);
        }
        return Ok(());
    }
    let dir = config_dir()?;
    let identity = Identity::load_or_create(&dir, "caw-desktop")?;
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    let port = std::env::var("CAW_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);

    let saved_settings = settings::Settings::load(&dir.join("settings.json"));
    #[cfg(target_os = "linux")]
    log(format!("typing and mouse through {}", linux::init(&dir).await.describe()));
    // macOS shows its dialog pointing to the Accessibility setting (once). Until the user
    // allows it, typing and the mouse report errors to the phone.
    // Headless, the app that started this receiver explains the permission itself.
    #[cfg(target_os = "macos")]
    if !typer::dry_run() && !headless::on() && !cgevent::ask_for_access() {
        log("not allowed to type or move the mouse yet: allow Caw Desktop in System Settings → Privacy & Security → Accessibility");
    }
    let state = Arc::new(State {
        config_dir: dir,
        name: name.clone(),
        fp: identity.fingerprint(),
        port,
        pairing: Mutex::new(None),
        pointer: pointer::Pointer::start().map_err(|e| log(format!("mouse control disabled: {e:#}"))).ok(),
        recent: Mutex::new(std::collections::VecDeque::new()),
        settings: Mutex::new(saved_settings),
        devices_changed: tokio::sync::watch::channel(0).0,
        phones_on: std::sync::atomic::AtomicBool::new(phones_on),
        pair_window_open: std::sync::atomic::AtomicBool::new(false),
        gate: tokio::sync::Notify::new(),
    });
    if let Some(ready) = ready {
        let _ = ready.send(state.clone());
    }

    if let Err(e) = fixes::ensure_file(&state.fixes_path()) {
        log(format!("word fixes disabled: {e:#}"));
    }

    let acceptor = TlsAcceptor::from(tlsconf::server_config(&identity)?);
    // Headless, the app talks to this receiver over stdin, and nothing else is
    // offered on the control socket: a standalone receiver started later then
    // finds the port taken instead of handing its menu to this one.
    let (stdin_tx, stdin_rx) = oneshot::channel();
    if headless::on() {
        tokio::spawn(headless_commands(state.clone(), stdin_tx));
    } else {
        drop(stdin_tx);
        tokio::spawn(control_loop(state.clone()));
    }
    let headless = headless::on();
    let stdin_closed = async move {
        if headless {
            let _ = stdin_rx.await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stdin_closed);

    if start_pairing {
        let (code, uri, rx) = state.start_pairing();
        show_pairing(&code, &uri);
        tokio::spawn(async move {
            let outcome = match timeout(PAIR_WINDOW, rx).await {
                Ok(Ok(o)) => o,
                _ => "expired".into(),
            };
            println!("{}", outcome_text(&outcome));
        });
    }

    let mut first = true;
    loop {
        // Phones turned off (the app): no port and no mDNS until they are on again or a pairing starts.
        if !state.receiving() {
            log("not listening for phones (\"Let phones type\" is off)");
            headless::emit(&headless::Event::Devices { devices: state.paired_phones() });
            while !state.receiving() {
                tokio::select! {
                    () = state.gate.notified() => {}
                    () = &mut stdin_closed => return Ok(()),
                }
            }
        }
        let listener = match listen(port) {
            Ok(l) => l,
            Err(e) if headless::on() && port_in_use(&e) => {
                log(format!("{e:#}"));
                headless::emit(&headless::Event::Stopped {
                    reason: "port_in_use".into(),
                    message: format!("another program is already using port {port}"),
                });
                std::process::exit(headless::EXIT_IN_USE);
            }
            Err(e) => return Err(e),
        };
        log(format!("listening on {}:{port} as {name:?} (id {})", local_ip(), short_id(&state.fp)));
        if first {
            first = false;
            headless::emit(&headless::Event::Listening {
                port,
                name: name.clone(),
                id: short_id(&state.fp),
                version: env!("CARGO_PKG_VERSION").into(),
                accessibility: may_type(),
            });
        }
        headless::emit(&headless::Event::Devices { devices: state.paired_phones() });
        // CAW_NO_MDNS=1 (for tests): the phone can only find the computer by the address in the QR code.
        let _mdns = if std::env::var_os("CAW_NO_MDNS").is_some() { None } else { advertise(&state).map_err(|e| log(format!("mDNS disabled: {e:#}"))).ok() };
        loop {
            let (tcp, mut addr) = tokio::select! {
                accepted = listener.accept() => accepted?,
                () = state.gate.notified() => {
                    if state.receiving() {
                        continue;
                    }
                    break;
                }
                () = &mut stdin_closed => {
                    log("the app closed stdin; stopping");
                    return Ok(());
                }
            };
            addr.set_ip(addr.ip().to_canonical());
            let (state, acceptor) = (state.clone(), acceptor.clone());
            tokio::spawn(async move {
                if let Err(e) = handle(state, acceptor, tcp, addr).await {
                    log(format!("{addr}: {e:#}"));
                }
            });
        }
    }
}

/// The mDNS advertisement, withdrawn when dropped.
struct Mdns(mdns_sd::ServiceDaemon);

impl Drop for Mdns {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

fn advertise(state: &State) -> Result<Mdns> {
    let daemon = mdns_sd::ServiceDaemon::new()?;
    let id = short_id(&state.fp);
    let props = [("id", id.as_str()), ("name", state.name.as_str()), ("v", "1")];
    let host = format!("{}.local.", state.name);
    let info = mdns_sd::ServiceInfo::new(SERVICE_TYPE, &state.name, &host, "", state.port, &props[..])?.enable_addr_auto();
    daemon.register(info)?;
    Ok(Mdns(daemon))
}

/// True when listening failed because something else has the port.
fn port_in_use(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse))
}

/// Whether this process may type and move the mouse (on a Mac: the
/// Accessibility permission of the app that started it).
fn may_type() -> bool {
    #[cfg(target_os = "macos")]
    {
        cgevent::allowed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Headless: commands from the app, one JSON object per line on stdin. Tells
/// `closed` when stdin ends.
async fn headless_commands(state: Arc<State>, closed: oneshot::Sender<()>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<headless::Command>(line) {
            Ok(cmd) => headless_command(&state, cmd),
            Err(e) => headless::emit(&headless::Event::Error { message: format!("unknown command: {e}") }),
        }
    }
    let _ = closed.send(());
}

fn headless_command(state: &Arc<State>, cmd: headless::Command) {
    use headless::{Command, Event};
    match cmd {
        Command::Pair => {
            let (code, uri, rx) = state.start_pairing();
            log("pairing code opened");
            headless::emit(&Event::Pairing { display: pairing::display(&code), code: code.clone(), uri, expires_in: PAIR_WINDOW.as_secs() });
            tokio::spawn(async move {
                let outcome = match timeout(PAIR_WINDOW, rx).await {
                    Ok(Ok(o)) => o,
                    _ => "expired".into(),
                };
                headless::emit(&Event::PairEnded { code, result: headless::pair_result(&outcome).into(), message: outcome });
            });
        }
        Command::CancelPair => state.cancel_pairing(),
        Command::Remove { id } => match state.remove_device(&id) {
            Ok(true) => log(format!("removed phone {id}")),
            Ok(false) => headless::emit(&Event::Error { message: format!("no paired phone with id {id}") }),
            Err(e) => headless::emit(&Event::Error { message: format!("removing the phone: {e:#}") }),
        },
        Command::Devices => headless::emit(&Event::Devices { devices: state.paired_phones() }),
    }
}

/// Local control socket used by `caw-desktop pair` while the receiver runs.
async fn control_loop(state: Arc<State>) {
    let mut listener = match control::Listener::bind() {
        Ok(l) => l,
        Err(e) => return log(format!("control channel disabled: {e}")),
    };
    loop {
        let Ok(stream) = listener.accept().await else { continue };
        let state = state.clone();
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(stream);
            let mut lines = BufReader::new(read).lines();
            if !matches!(lines.next_line().await, Ok(Some(l)) if l.trim() == "pair") {
                return;
            }
            let (code, uri, rx) = state.start_pairing();
            let first = serde_json::to_string(&(code, uri)).unwrap_or_default();
            if write.write_all(format!("{first}\n").as_bytes()).await.is_err() {
                return;
            }
            let outcome = match timeout(PAIR_WINDOW, rx).await {
                Ok(Ok(o)) => o,
                _ => "expired".into(),
            };
            let _ = write.write_all(format!("{outcome}\n").as_bytes()).await;
        });
    }
}

async fn handle(state: Arc<State>, acceptor: TlsAcceptor, tcp: TcpStream, addr: SocketAddr) -> Result<()> {
    let tls = timeout(Duration::from_secs(10), acceptor.accept(tcp)).await.context("TLS handshake timed out")??;
    let peer_fp = {
        let (_, conn) = tls.get_ref();
        let certs = conn.peer_certificates().context("phone sent no certificate")?;
        fingerprint(certs.first().context("phone sent no certificate")?.as_ref())
    };
    let mut io = Framed::new(tls, LinesCodec::new_with_max_length(MAX_LINE));
    let mut authed = false;
    let mut session = String::new();
    let mut warned_no_pointer = false;
    let mut _connected = None;
    let mut devices_changed = state.devices_changed.subscribe();

    loop {
        let next = tokio::select! {
            next = timeout(IDLE_TIMEOUT, io.next()) => next,
            Ok(()) = devices_changed.changed(), if authed => {
                if state.is_trusted(&peer_fp) && state.receiving() {
                    continue;
                }
                let _ = send(&mut io, Msg::Error { id: None, reason: "not_paired".into() }).await;
                bail!("the phone was removed, disconnecting");
            }
        };
        let line = match next {
            Err(_) => bail!("no messages for {}s, disconnecting", IDLE_TIMEOUT.as_secs()),
            Ok(None) => return Ok(()),
            // Phones often drop the socket without a TLS goodbye; that's a normal disconnect.
            Ok(Some(Err(tokio_util::codec::LinesCodecError::Io(e)))) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Some(line)) => line?,
        };
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) if authed => {
                send(&mut io, Msg::Error { id: None, reason: "bad_json".into() }).await?;
                continue;
            }
            Err(_) => bail!("bad JSON before hello"),
        };
        let msg = match serde_json::from_value::<Msg>(value.clone()) {
            Ok(m) => m,
            Err(_) if authed => {
                let id = value.get("id").and_then(|v| v.as_u64());
                send(&mut io, Msg::Error { id, reason: "unknown_type".into() }).await?;
                continue;
            }
            Err(_) => bail!("unexpected message before hello"),
        };

        match msg {
            Msg::PairRequest { device_id, device_name, proof, .. } if !authed => match state.try_pair(&peer_fp, &device_id, &device_name, &proof) {
                Ok(reply) => {
                    log(format!("{addr}: paired with {device_name:?} ({device_id})"));
                    let desktop_id = short_id(&state.fp);
                    send(&mut io, Msg::PairOk { desktop_id, desktop_name: state.name.clone(), proof: B64.encode(reply) }).await?;
                }
                Err(reason) => {
                    send(&mut io, Msg::PairError { reason: reason.into() }).await?;
                    bail!("pairing refused: {reason}");
                }
            },
            Msg::Hello { v, session: s, .. } if !authed => {
                if !state.is_trusted(&peer_fp) {
                    send(&mut io, Msg::Error { id: None, reason: "not_paired".into() }).await?;
                    bail!("unknown phone, disconnecting");
                }
                if v > VERSION {
                    log(format!("{addr}: phone speaks protocol v{v}, this is v{VERSION}"));
                }
                authed = true;
                session = s.unwrap_or_default();
                _connected = Some(Connected::new(short_id(&peer_fp), state.phone_name(&peer_fp)));
                log(format!("{addr}: connected ({})", short_id(&peer_fp)));
                send(&mut io, Msg::HelloOk { desktop_name: state.name.clone() }).await?;
            }
            _ if !authed => bail!("unexpected message before hello"),
            Msg::Ping => send(&mut io, Msg::Pong).await?,
            Msg::Pong => {}
            Msg::PointerMove { dx, dy } => {
                let s = *state.settings.lock().unwrap();
                let (dx, dy) = pointer::accelerate(dx, dy, s.pointer_speed, s.pointer_accel);
                pointer_cmd(&state, &mut io, &mut warned_no_pointer, None, pointer::Cmd::Move { dx, dy }).await?;
            }
            Msg::Scroll { dx, dy } => {
                pointer_cmd(&state, &mut io, &mut warned_no_pointer, None, pointer::Cmd::Scroll { dx, dy }).await?;
            }
            Msg::Click { id, button, action } => {
                let Ok(button) = pointer::Button::parse(&button) else {
                    send(&mut io, Msg::Error { id, reason: "unknown_button".into() }).await?;
                    continue;
                };
                let presses: &[bool] = match action.as_str() {
                    "down" => &[true],
                    "up" => &[false],
                    _ => &[true, false],
                };
                let mut ok = true;
                for &pressed in presses {
                    ok &= pointer_cmd(&state, &mut io, &mut warned_no_pointer, id, pointer::Cmd::Button { button, pressed }).await?;
                }
                if let (Some(id), true) = (id, ok) {
                    send(&mut io, Msg::Ack { id }).await?;
                }
            }
            ref m @ (Msg::Text { id, .. } | Msg::Key { id, .. }) if state.already_handled(&peer_fp, &session, m) => {
                log(format!("{addr}: skipped resent message {id}"));
                send(&mut io, Msg::Ack { id }).await?;
            }
            Msg::SettingsGet { id } => {
                let s = *state.settings.lock().unwrap();
                send(&mut io, Msg::Settings {
                    id,
                    desktop_name: state.name.clone(),
                    desktop_version: env!("CARGO_PKG_VERSION").into(),
                    word_fixes: fixes::load(&state.fixes_path()),
                    pointer_speed: s.pointer_speed,
                    pointer_accel: s.pointer_accel,
                    shortcuts: shortcuts::load(&state.shortcuts_path()),
                })
                .await?;
            }
            Msg::SettingsSet { id, word_fixes, pointer_speed, pointer_accel, shortcuts } => {
                let reply = match state.update_settings(word_fixes, pointer_speed, pointer_accel, shortcuts) {
                    Ok(()) => {
                        log(format!("{addr}: settings updated from the phone"));
                        Msg::Ack { id }
                    }
                    Err(e) => Msg::Error { id: Some(id), reason: format!("invalid_settings: {e}") },
                };
                send(&mut io, reply).await?;
            }
            Msg::Text { id, text } => {
                let original = Msg::Text { id, text: text.clone() };
                let result = {
                    let _turn = typer::TURN.lock().await;
                    let text = fixes::apply(&text, &fixes::load(&state.fixes_path()));
                    typer::type_text(&text).await
                };
                if result.is_err() {
                    state.forget(&peer_fp, &session, &original);
                }
                send(&mut io, ack_or_error(id, result)).await?;
            }
            Msg::Key { id, key, mods, count } => {
                let original = Msg::Key { id, key: key.clone(), mods: mods.clone(), count };
                let Ok((key, mods)) = typer::parse_combo(&key, &mods) else {
                    state.forget(&peer_fp, &session, &original);
                    send(&mut io, Msg::Error { id: Some(id), reason: "unknown_key".into() }).await?;
                    continue;
                };
                let result = {
                    let _turn = typer::TURN.lock().await;
                    typer::press_key(key, &mods, count).await
                };
                if result.is_err() {
                    state.forget(&peer_fp, &session, &original);
                }
                send(&mut io, ack_or_error(id, result)).await?;
            }
            other => {
                let id = serde_json::to_value(&other).ok().and_then(|v| v.get("id").and_then(|i| i.as_u64()));
                send(&mut io, Msg::Error { id, reason: "unexpected".into() }).await?;
            }
        }
    }
}

/// Sends a mouse command; reports "pointer_unavailable" once per connection.
async fn pointer_cmd<T>(state: &State, io: &mut Framed<T, LinesCodec>, warned: &mut bool, id: Option<u64>, cmd: pointer::Cmd) -> Result<bool>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if state.pointer.as_ref().is_some_and(|p| p.send(cmd).is_ok()) {
        return Ok(true);
    }
    if id.is_some() || !*warned {
        *warned = true;
        send(io, Msg::Error { id, reason: "pointer_unavailable".into() }).await?;
    }
    Ok(false)
}

fn ack_or_error(id: u64, result: Result<()>) -> Msg {
    match result {
        Ok(()) => Msg::Ack { id },
        Err(e) => {
            log(format!("typing failed: {e:#}"));
            Msg::Error { id: Some(id), reason: "typing_failed".into() }
        }
    }
}

async fn send<T>(io: &mut Framed<T, LinesCodec>, msg: Msg) -> Result<()>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    io.send(serde_json::to_string(&msg)?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarted_app_is_not_a_resend() {
        let fp = [7u8; 32];
        let enter = Msg::Key { id: 1, key: "enter".into(), mods: vec![], count: 1 };
        assert_eq!(dedup_key(&fp, "a", &enter), dedup_key(&fp, "a", &enter));
        assert_ne!(dedup_key(&fp, "a", &enter), dedup_key(&fp, "b", &enter));
        // Super+Enter with the same id is a different message, not a resend of Enter.
        let super_enter = Msg::Key { id: 1, key: "enter".into(), mods: vec!["super".into()], count: 1 };
        assert_ne!(dedup_key(&fp, "a", &enter), dedup_key(&fp, "a", &super_enter));
    }
}
