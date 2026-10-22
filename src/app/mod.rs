//! `caw-desktop app`: the phone receiver and hold-to-talk dictation in one
//! process, with a tray icon, the floating Crumb (click him to talk) and
//! windows for word fixes, pairing a phone and settings (GTK 4).
//!
//! The receiver and dictation run on the tokio runtime as they do on their
//! own; their events come to the GTK main loop through one channel. Dictated
//! text is kept in memory only (the "Last dictation" in Word fixes).

mod crumb;
mod fixes_window;
mod orb;
mod pair_window;
pub mod personal;
mod prefs;
mod settings_window;
mod talk_key;
mod tray;
mod webkit;

use crate::dictate::{self, binding, Activity, Daemon};
use crate::headless;
use crate::State;
use anyhow::{bail, Result};
use gtk::prelude::*;
use gtk::{gio, glib};
use prefs::Prefs;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const APP_ID: &str = "dev.birdbrainlabs.CawDesktop";

pub(crate) fn log(msg: impl AsRef<str>) {
    eprintln!("caw-desktop: {}", msg.as_ref());
}

/// From the tray menu.
#[derive(Clone, Debug)]
pub enum Command {
    SetDictation(bool),
    SetPhones(bool),
    OpenFixes,
    OpenPair,
    OpenSettings,
    Quit,
}

/// Everything that reaches the GTK main loop from elsewhere.
enum Msg {
    Dictation(dictate::Event),
    DictationReady(Arc<Daemon>),
    DictationStopped(String),
    Receiver(headless::Event),
    ReceiverReady(Arc<State>),
    ReceiverStopped(String),
    Tray(ksni::Handle<tray::Tray>),
    Command(Command),
    TalkKey(talk_key::Status),
}

/// An open pairing code.
#[derive(Clone)]
pub struct Pairing {
    pub code: String,
    pub display: String,
    pub uri: String,
    pub expires: Instant,
}

pub struct Ui {
    app: gtk::Application,
    rt: tokio::runtime::Handle,
    config_dir: PathBuf,
    prefs: RefCell<Prefs>,
    daemon: RefCell<Option<Arc<Daemon>>>,
    dictation_problem: RefCell<Option<String>>,
    activity: Cell<Activity>,
    receiver: RefCell<Option<Arc<State>>>,
    receiver_problem: RefCell<Option<String>>,
    /// (id, name) of the phones connected now.
    connected: RefCell<Vec<(String, String)>>,
    pairing: RefCell<Option<Pairing>>,
    pair_outcome: RefCell<Option<String>>,
    /// The last dictation's text; memory only.
    last_text: RefCell<Option<String>>,
    tray: RefCell<Option<ksni::Handle<tray::Tray>>>,
    tray_view: RefCell<tray::View>,
    crumb: RefCell<Option<Rc<crumb::Crumb>>>,
    fixes: RefCell<Option<Rc<fixes_window::FixesWindow>>>,
    pair: RefCell<Option<Rc<pair_window::PairWindow>>>,
    settings: RefCell<Option<Rc<settings_window::SettingsWindow>>>,
    /// "Hold Fn to talk", from the desktop's key binding.
    talk_hint: RefCell<String>,
    /// The talk key through the desktop's shortcuts portal (not on Hyprland): what the desktop said.
    talk_status: RefCell<Option<talk_key::Status>>,
    /// Asks the portal for another talk key.
    rebind: RefCell<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    msgs: async_channel::Sender<Msg>,
}

/// `caw-desktop app`; `pair` (from `pair --window`) opens "Pair a phone" once it is up.
pub fn run(runtime: tokio::runtime::Runtime, pair: bool) -> Result<()> {
    let config_dir = crate::config_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    if gtk::init().is_err() {
        bail!("the app needs a graphical session (no display found)");
    }
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.register(gio::Cancellable::NONE)?;
    if app.is_remote() {
        // Opened again while it runs: the running one shows "Pair a phone" (run
        // asks it to, then returns at once).
        app.run_with_args::<&str>(&[]);
        return Ok(());
    }

    let prefs = Prefs::load(&config_dir);
    let (tx, rx) = async_channel::unbounded::<Msg>();
    {
        let tx = tx.clone();
        headless::set_hook(Box::new(move |event| {
            let _ = tx.try_send(Msg::Receiver(event.clone()));
        }));
    }
    // GNOME's portals want to know which app is asking (for the talk key above
    // all), and a host app must say so before its first portal call. Hyprland
    // needs neither, so it is left as it was.
    if !binding::on_hyprland() {
        runtime.block_on(register_app());
    }
    // Once, for both: on GNOME and KDE this may ask for permission.
    let input = runtime.block_on(crate::linux::init(&config_dir));
    log(format!("typing and mouse through {}", input.describe()));

    stop_old_receiver(&runtime);
    // The phone receiver.
    {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let tx2 = tx.clone();
        runtime.spawn(async move {
            if let Ok(state) = ready_rx.await {
                let _ = tx2.send(Msg::ReceiverReady(state)).await;
            }
        });
        let tx = tx.clone();
        let phones_on = prefs.phones_type;
        runtime.spawn(async move {
            let why = match crate::serve_with(crate::ServeOptions { start_pairing: false, phones_on, ready: Some(ready_tx) }).await {
                Ok(()) => "Another Caw Desktop is already receiving from phones on this computer".to_string(),
                Err(e) => format!("The phone receiver stopped: {e:#}"),
            };
            log(&why);
            let _ = tx.send(Msg::ReceiverStopped(why)).await;
        });
    }
    // Dictation.
    {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let tx2 = tx.clone();
        runtime.spawn(async move {
            if let Ok(daemon) = ready_rx.await {
                let _ = tx2.send(Msg::DictationReady(daemon)).await;
            }
        });
        let hook_tx = tx.clone();
        let opts = dictate::Options {
            enabled: prefs.dictation,
            hook: Box::new(move |event| {
                let _ = hook_tx.try_send(Msg::Dictation(event));
            }),
            ready: Some(ready_tx),
            signals: false,
        };
        let tx = tx.clone();
        let dir = config_dir.clone();
        runtime.spawn(async move {
            let why = match dictate::run_with(dir, opts).await {
                Ok(()) => "Dictation stopped".to_string(),
                Err(e) => format!("Dictation isn't running: {e:#}"),
            };
            log(&why);
            let _ = tx.send(Msg::DictationStopped(why)).await;
        });
    }
    // The tray icon.
    {
        let tx = tx.clone();
        let (cmd_tx, cmd_rx) = async_channel::unbounded::<Command>();
        let own_icons = gtk::gdk::Display::default().is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(tray::OWN_ICON));
        let tray = tray::Tray { view: tray::View::default(), commands: cmd_tx, own_icons };
        runtime.spawn(async move {
            use ksni::TrayMethods;
            match tray.spawn().await {
                Ok(handle) => {
                    let _ = tx.send(Msg::Tray(handle)).await;
                }
                Err(e) => log(format!("no tray icon: {e}")),
            }
            while let Ok(command) = cmd_rx.recv().await {
                let _ = tx.send(Msg::Command(command)).await;
            }
        });
    }

    let ui = Rc::new(Ui {
        app: app.clone(),
        rt: runtime.handle().clone(),
        config_dir,
        prefs: RefCell::new(prefs),
        daemon: RefCell::new(None),
        dictation_problem: RefCell::new(None),
        activity: Cell::new(Activity::Idle),
        receiver: RefCell::new(None),
        receiver_problem: RefCell::new(None),
        connected: RefCell::new(Vec::new()),
        pairing: RefCell::new(None),
        pair_outcome: RefCell::new(None),
        last_text: RefCell::new(None),
        tray: RefCell::new(None),
        tray_view: RefCell::new(tray::View::default()),
        crumb: RefCell::new(None),
        fixes: RefCell::new(None),
        pair: RefCell::new(None),
        settings: RefCell::new(None),
        talk_hint: RefCell::new(String::new()),
        talk_status: RefCell::new(None),
        rebind: RefCell::new(None),
        msgs: tx.clone(),
    });
    ui.refresh_talk_hint();
    load_css();

    let first = Cell::new(true);
    let weak = Rc::downgrade(&ui);
    app.connect_activate(move |_| {
        if first.replace(false) {
            return;
        }
        if let Some(ui) = weak.upgrade() {
            pair_window::open(&ui);
        }
    });
    let weak = Rc::downgrade(&ui);
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            let Some(ui) = weak.upgrade() else { break };
            ui.handle(msg);
        }
    });
    // SIGTERM (systemd stop), SIGINT and SIGHUP: quit cleanly, as Quit in the menu.
    {
        let tx = tx.clone();
        runtime.spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup())) else { return };
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
                _ = hup.recv() => {}
            }
            let _ = tx.send(Msg::Command(Command::Quit)).await;
        });
    }
    // Every 10 s: the pairing countdown and anything else that ages.
    let weak = Rc::downgrade(&ui);
    glib::timeout_add_seconds_local(10, move || {
        let Some(ui) = weak.upgrade() else { return glib::ControlFlow::Break };
        ui.update_tray();
        glib::ControlFlow::Continue
    });

    if pair {
        let ui = ui.clone();
        glib::idle_add_local_once(move || pair_window::open(&ui));
    }
    let _hold = app.hold();
    app.run_with_args::<&str>(&[]);

    // Leave no sockets behind for the next start to trip over.
    if ui.daemon.borrow().is_some() {
        let _ = std::fs::remove_file(dictate::socket_path());
    }
    if ui.receiver.borrow().is_some() {
        crate::control::remove();
    }
    if let Some(crumb) = ui.crumb.borrow().as_ref() {
        crumb.hide();
    }
    runtime.shutdown_timeout(Duration::from_millis(500));
    std::process::exit(0);
}

/// After an upgrade from the receiver-only Caw Desktop, its receiver
/// (`caw-desktop serve`, started at login) may still be running. The app
/// receives from phones itself, so that one is asked to stop (SIGTERM). Both
/// read the same config folder, so paired phones stay paired.
fn stop_old_receiver(runtime: &tokio::runtime::Runtime) {
    use std::os::unix::fs::MetadataExt;
    if runtime.block_on(crate::control::connect()).is_err() {
        return;
    }
    let me = std::process::id();
    let uid = unsafe { libc::getuid() };
    let mut stopped = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if pid == me || entry.metadata().map(|m| m.uid()).ok() != Some(uid) {
            continue;
        }
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).filter(|a| !a.is_empty()).collect();
        let is_receiver = args.len() == 2 && args[1] == b"serve" && args[0].rsplit(|&b| b == b'/').next() == Some(b"caw-desktop");
        if is_receiver && unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
            stopped.push(pid);
        }
    }
    if stopped.is_empty() {
        return;
    }
    log(format!("stopped the receiver that was already running (pid {stopped:?}); this app receives from phones now"));
    for _ in 0..30 {
        if runtime.block_on(crate::control::connect()).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Tells the portals this process is the app with APP_ID (its .desktop file has that name).
async fn register_app() {
    let Ok(id) = ashpd::AppID::try_from(APP_ID) else { return };
    match ashpd::register_host_app(id).await {
        Ok(()) => log(format!("known to the desktop's portals as {APP_ID}")),
        Err(e) => log(format!("couldn't register with the desktop's portals as {APP_ID} ({e}); some may refuse requests")),
    }
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        "window.caw-crumb, window.caw-crumb > overlay, window.caw-crumb stack { background: none; box-shadow: none; }
         .caw-message { background-color: rgba(38, 39, 46, 0.94); border: 1px solid rgba(255, 255, 255, 0.16); border-radius: 999px; padding: 0 14px; margin: 4px; color: #fff; }
         .caw-message image { color: #e9694f; }
         .caw-message-text { font-size: 12px; font-weight: 600; }
         .caw-code { font-family: monospace; font-size: 24px; font-weight: 600; }
         .caw-dot-on { color: #2ec27e; }
         .caw-dot-off { color: alpha(currentColor, 0.35); }
         .caw-banner { background-color: alpha(#e9694f, 0.15); border-radius: 8px; padding: 10px; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

impl Ui {
    fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Dictation(event) => self.dictation_event(event),
            Msg::DictationReady(daemon) => {
                *self.daemon.borrow_mut() = Some(daemon.clone());
                self.start_talk_key(daemon);
                self.update_crumb();
            }
            Msg::DictationStopped(why) => {
                *self.daemon.borrow_mut() = None;
                *self.dictation_problem.borrow_mut() = Some(why);
                self.update_crumb();
            }
            Msg::Receiver(event) => self.receiver_event(event),
            Msg::ReceiverReady(state) => {
                *self.receiver.borrow_mut() = Some(state);
                if let Some(pair) = self.pair.borrow().clone().filter(|p| p.is_open()) {
                    pair.opened(self);
                }
            }
            Msg::ReceiverStopped(why) => {
                *self.receiver.borrow_mut() = None;
                *self.receiver_problem.borrow_mut() = Some(why);
                self.connected.borrow_mut().clear();
            }
            Msg::Tray(handle) => {
                *self.tray.borrow_mut() = Some(handle);
                *self.tray_view.borrow_mut() = tray::View::default();
            }
            Msg::Command(command) => self.command(command),
            Msg::TalkKey(status) => {
                *self.talk_status.borrow_mut() = Some(status);
                self.refresh_talk_hint();
            }
        }
        self.update_tray();
        self.refresh_windows();
    }

    fn dictation_event(self: &Rc<Self>, event: dictate::Event) {
        match event {
            dictate::Event::Activity(activity) => {
                self.activity.set(activity);
                if let Some(crumb) = self.crumb.borrow().as_ref() {
                    crumb.set_activity(activity);
                }
            }
            dictate::Event::Heard(text) => {
                *self.last_text.borrow_mut() = Some(text);
                if let Some(fixes) = self.fixes.borrow().as_ref() {
                    fixes.show_last(self);
                }
            }
            dictate::Event::Typed => {
                if let Some(crumb) = self.crumb.borrow().as_ref() {
                    crumb.delivered();
                }
            }
            dictate::Event::Failed(why) => {
                if let Some(crumb) = self.crumb.borrow().as_ref().filter(|_| self.crumb_wanted()) {
                    crumb.warn(&why);
                }
            }
        }
    }

    fn receiver_event(self: &Rc<Self>, event: headless::Event) {
        use headless::Event;
        match event {
            Event::Connected { id, name } => self.connected.borrow_mut().push((id, name)),
            Event::Disconnected { id, .. } => {
                let mut list = self.connected.borrow_mut();
                if let Some(i) = list.iter().position(|(i, _)| *i == id) {
                    list.remove(i);
                }
            }
            Event::Pairing { code, display, uri, expires_in } => {
                *self.pairing.borrow_mut() = Some(Pairing { code, display, uri, expires: Instant::now() + Duration::from_secs(expires_in) });
                *self.pair_outcome.borrow_mut() = None;
            }
            Event::PairEnded { code, result, message } => {
                let current = self.pairing.borrow().as_ref().is_some_and(|p| p.code == code);
                if current {
                    *self.pairing.borrow_mut() = None;
                    *self.pair_outcome.borrow_mut() = Some(match result.as_str() {
                        "paired" => format!("{}. It can type on this computer now.", upper_first(&message)),
                        "expired" => "This code has expired.".into(),
                        "cancelled" | "replaced" => String::new(),
                        _ => format!("Pairing didn't work: {message}."),
                    })
                    .filter(|s| !s.is_empty());
                }
                // As on the Mac: pairing a phone turns "Let phones type" on.
                if result == "paired" && !self.prefs.borrow().phones_type {
                    self.set_phones(true);
                }
            }
            Event::Stopped { message, .. } | Event::Error { message } => log(message),
            Event::Listening { .. } | Event::Devices { .. } => {}
        }
    }

    fn command(self: &Rc<Self>, command: Command) {
        match command {
            Command::SetDictation(on) => self.set_dictation(on),
            Command::SetPhones(on) => self.set_phones(on),
            Command::OpenFixes => fixes_window::open(self),
            Command::OpenPair => pair_window::open(self),
            Command::OpenSettings => settings_window::open(self),
            Command::Quit => self.app.quit(),
        }
    }

    fn save_prefs(&self) {
        self.prefs.borrow().save(&self.config_dir);
    }

    pub fn set_dictation(self: &Rc<Self>, on: bool) {
        self.prefs.borrow_mut().dictation = on;
        self.save_prefs();
        if let Some(daemon) = self.daemon.borrow().clone() {
            daemon.set_enabled(on);
            if !on && self.activity.get() == Activity::Listening {
                self.rt.spawn(async move {
                    daemon.stop();
                });
            }
        }
        self.update_crumb();
        self.update_tray();
        self.refresh_windows();
    }

    pub fn set_phones(self: &Rc<Self>, on: bool) {
        self.prefs.borrow_mut().phones_type = on;
        self.save_prefs();
        if let Some(state) = self.receiver.borrow().as_ref() {
            state.set_phones(on);
        }
        if !on {
            self.connected.borrow_mut().clear();
        }
        self.update_tray();
        self.refresh_windows();
    }

    pub fn set_show_crumb(self: &Rc<Self>, on: bool) {
        self.prefs.borrow_mut().show_crumb = on;
        self.save_prefs();
        self.update_crumb();
    }

    fn crumb_wanted(&self) -> bool {
        let prefs = self.prefs.borrow();
        prefs.show_crumb && prefs.dictation && self.daemon.borrow().is_some() && crumb::supported()
    }

    /// Shows or hides the floating Crumb as the switches say.
    fn update_crumb(self: &Rc<Self>) {
        let wanted = self.crumb_wanted();
        dictate::set_quiet(wanted);
        if wanted && self.crumb.borrow().is_none() {
            let anchor = self.prefs.borrow().crumb_at;
            let click = Rc::downgrade(self);
            let moved = Rc::downgrade(self);
            let level = self.daemon.borrow().clone();
            let crumb = crumb::Crumb::new(
                &self.app,
                anchor,
                crumb::Hooks {
                    click: Box::new(move || {
                        if let Some(ui) = click.upgrade() {
                            ui.crumb_clicked();
                        }
                    }),
                    moved: Box::new(move |at| {
                        if let Some(ui) = moved.upgrade() {
                            ui.prefs.borrow_mut().crumb_at = Some(at);
                            ui.save_prefs();
                        }
                    }),
                    level: Box::new(move || level.as_ref().map(|d| d.level()).unwrap_or(0.0)),
                },
            );
            *self.crumb.borrow_mut() = Some(crumb);
        }
        if let Some(crumb) = self.crumb.borrow().as_ref() {
            if wanted {
                crumb.show();
                crumb.set_activity(self.activity.get());
            } else {
                crumb.hide();
            }
        }
    }

    /// As on the Mac: listening, it stops; idle, it starts; while a dictation is being finished, nothing.
    fn crumb_clicked(&self) {
        let Some(daemon) = self.daemon.borrow().clone() else { return };
        let activity = daemon.activity();
        self.rt.spawn(async move {
            match activity {
                Activity::Listening => {
                    daemon.stop();
                }
                Activity::Idle => {
                    let _ = daemon.start_reporting();
                }
                Activity::Working => {}
            }
        });
    }

    fn refresh_talk_hint(&self) {
        let hint = if binding::on_hyprland() {
            match binding::find(&binding::hypr_dir()) {
                Some(b) => format!("Hold {} to talk", binding::talk_key_for(&b.keys).map(binding::describe).unwrap_or(b.keys)),
                None => "No talk key yet: see Settings".into(),
            }
        } else {
            match self.talk_status.borrow().as_ref() {
                Some(talk_key::Status::Bound(name)) => format!("Hold {} to talk", self.talk_key_name(name)),
                Some(talk_key::Status::Asking) => "Waiting for the desktop to allow the talk key".into(),
                _ if crumb::supported() => "Click Crumb to talk, or bind a key in your desktop's settings".into(),
                _ => "No talk key yet: see Settings".into(),
            }
        };
        *self.talk_hint.borrow_mut() = hint;
    }

    /// The talk key's name for people: the desktop's, else Caw's ("Right Ctrl").
    /// The talk key as the desktop describes it (GNOME: "Press Control_R"), in
    /// Caw's words ("Right Ctrl") when it is the key Caw asked for.
    fn talk_key_name(&self, from_desktop: &str) -> String {
        let chosen = dictate::Settings::load(&self.config_dir).talk_key();
        let said = from_desktop.trim();
        let said = said.strip_prefix("Press ").unwrap_or(said).trim();
        if said.is_empty() || said == talk_key::trigger(&chosen) {
            binding::describe(&chosen)
        } else {
            said.to_string()
        }
    }

    /// Off Hyprland: the talk key through the desktop's global-shortcuts portal.
    fn start_talk_key(&self, daemon: Arc<Daemon>) {
        if binding::on_hyprland() || self.rebind.borrow().is_some() {
            return;
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.rebind.borrow_mut() = Some(tx);
        let key = dictate::Settings::load(&self.config_dir).talk_key();
        let msgs = self.msgs.clone();
        self.rt.spawn(talk_key::run(daemon, key, rx, move |status| {
            let _ = msgs.try_send(Msg::TalkKey(status));
        }));
    }

    /// Settings chose another talk key: ask the desktop for it. False on Hyprland (no portal there).
    pub fn rebind_talk_key(&self, talk_key: &str) -> bool {
        match self.rebind.borrow().as_ref() {
            Some(tx) => tx.send(talk_key.to_string()).is_ok(),
            None => false,
        }
    }

    fn tray_view(&self) -> tray::View {
        let prefs = self.prefs.borrow();
        let dictation_line = if let Some(problem) = self.dictation_problem.borrow().as_ref().filter(|_| self.daemon.borrow().is_none()) {
            problem.clone()
        } else if !prefs.dictation {
            "Dictation is off".into()
        } else if self.daemon.borrow().is_none() {
            "Dictation is starting…".into()
        } else {
            match self.activity.get() {
                Activity::Listening => "Listening…".into(),
                Activity::Working => "Typing what you said…".into(),
                Activity::Idle => self.talk_hint.borrow().clone(),
            }
        };
        let connected = self.connected.borrow();
        let phone_line = if let Some(problem) = self.receiver_problem.borrow().as_ref().filter(|_| self.receiver.borrow().is_none()) {
            problem.clone()
        } else if !prefs.phones_type {
            "Phones can't type (turned off)".into()
        } else if connected.is_empty() {
            "No phone connected".into()
        } else {
            format!("Connected: {}", connected.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", "))
        };
        tray::View {
            dictation_on: prefs.dictation,
            listening: self.activity.get() == Activity::Listening,
            dictation_line,
            phones_on: prefs.phones_type,
            phone_line,
            phone_connected: !connected.is_empty(),
        }
    }

    fn update_tray(&self) {
        let view = self.tray_view();
        if *self.tray_view.borrow() == view {
            return;
        }
        *self.tray_view.borrow_mut() = view.clone();
        if let Some(handle) = self.tray.borrow().clone() {
            self.rt.spawn(async move {
                handle.update(move |t| t.view = view).await;
            });
        }
    }

    fn refresh_windows(self: &Rc<Self>) {
        if let Some(w) = self.pair.borrow().clone() {
            w.refresh(self);
        }
        if let Some(w) = self.settings.borrow().clone() {
            w.refresh(self);
        }
    }

    /// Runs one of the receiver's commands (pair, cancel, remove) on its runtime.
    fn receiver_command(&self, cmd: headless::Command) {
        let Some(state) = self.receiver.borrow().clone() else { return };
        self.rt.spawn(async move { crate::headless_command(&state, cmd) });
    }
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
