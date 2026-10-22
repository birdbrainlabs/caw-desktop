//! Hold-to-talk dictation on this computer: `caw-desktop dictate` runs a small
//! daemon that keeps the speech model loaded; `caw-desktop talk start` /
//! `talk stop` (bound by the desktop to a key's press and release) record
//! from the microphone, transcribe locally and type the text at the cursor.
//! Caw never reads the keyboard itself, and nothing here uses the network.
//!
//! Hold rules, as in the phone app: the key may be held up to 5 minutes and
//! pauses of any length are fine; after release the microphone keeps running
//! for 0.4 s (pressing again then just carries on). Return is never pressed.
//! If transcription or typing fails, the recording and any text are saved in
//! ~/.local/share/caw-desktop/unsent/; otherwise no recording is kept.

#![cfg_attr(not(feature = "app-base"), allow(dead_code))]

mod audio;
pub mod binding;
mod recognizer;

use crate::{fixes, typer};
use anyhow::{bail, Context, Result};
use caw_desktop::speech::{cleaner, filter::HighPass, segmenter, wordfix};
use recognizer::{Moonshine, Segmented, Timing, SAMPLE_RATE};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

const MAX_HOLD: Duration = Duration::from_secs(300);
const TAIL: Duration = Duration::from_millis(400);
/// A hold that gives no words is saved as unsent only if some 100 ms of it
/// (after its first 0.2 s) reached this RMS, i.e. someone probably spoke.
/// Measured: a quiet room on a laptop microphone is about 0.01 after the high-pass.
const SPEECH_LEVEL: f32 = 0.02;

// ---- settings ----

/// ~/.config/caw-desktop/dictation.json. Every field is optional.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Settings {
    /// The hold-to-talk key: "ctrl_r" (Right Ctrl), "alt_r", "super_r", "fn",
    /// a keysym name such as "F13", or "code:<n>" for a raw key code. Unset:
    /// Fn on Apple keyboards, Right Ctrl on others (binding::default_talk_key).
    /// Caw doesn't watch the key itself; the desktop's shortcut runs
    /// `caw-desktop talk start` / `talk stop` (see `caw-desktop talk bindings`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub talk_key: Option<String>,
    /// The folder with the Moonshine model files. Default:
    /// ~/.local/share/caw-desktop/models/moonshine-base-en if it is there, else
    /// the one the .deb installs (PACKAGED_MODEL). CAW_MODEL_DIR overrides it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { talk_key: None, model_dir: None }
    }
}

impl Settings {
    pub fn load(config_dir: &Path) -> Self {
        std::fs::read(config_dir.join("dictation.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    #[cfg_attr(not(feature = "app-base"), allow(dead_code))]
    pub fn save(&self, config_dir: &Path) -> Result<()> {
        crate::fsutil::write_private(&config_dir.join("dictation.json"), &serde_json::to_vec_pretty(self)?)
    }

    pub fn talk_key(&self) -> String {
        self.talk_key.clone().filter(|k| !k.trim().is_empty()).unwrap_or_else(|| binding::default_talk_key().into())
    }

    fn model_dir(&self) -> PathBuf {
        if let Some(dir) = std::env::var_os("CAW_MODEL_DIR") {
            return dir.into();
        }
        if let Some(dir) = &self.model_dir {
            return dir.clone();
        }
        let own = data_dir().join("models").join("moonshine-base-en");
        if !own.is_dir() && Path::new(PACKAGED_MODEL).is_dir() {
            return PACKAGED_MODEL.into();
        }
        own
    }
}

/// Where the .deb puts the speech model (it ships inside the package; Caw never downloads it).
pub const PACKAGED_MODEL: &str = "/usr/share/caw-desktop/models/moonshine-base-en";

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("caw-desktop")
}

pub fn socket_path() -> PathBuf {
    match std::env::var_os("CAW_DICTATE_SOCKET") {
        Some(p) => p.into(),
        None => dirs::runtime_dir().unwrap_or_else(std::env::temp_dir).join("caw-dictate.sock"),
    }
}

/// The word fixes for one hold, read fresh so edits apply at once: first the
/// personal list (word-fixes.personal.json, in the Caw for Mac / Android
/// document format), then the phone-managed word-fixes.json ({heard: fixed}).
/// That is the order a phone dictation gets them: the phone applies its own
/// list, then this computer applies word-fixes.json to the text it receives.
fn word_fixes(config_dir: &Path) -> impl Fn(&str) -> String {
    let path = config_dir.join("word-fixes.personal.json");
    let personal = match std::fs::read_to_string(&path) {
        Ok(json) => match wordfix::parse(&json) {
            Ok(list) => {
                let compiled = wordfix::WordFixes::new(list);
                if !compiled.invalid.is_empty() {
                    log(format!("{}: skipped {} fix(es) that could not be read", path.display(), compiled.invalid.len()));
                }
                compiled
            }
            Err(e) => {
                log(format!("{} could not be read ({e}); dictating without it", path.display()));
                notify(&format!("Your word fixes could not be read ({e}). Dictating without them."));
                wordfix::WordFixes::default()
            }
        },
        Err(_) => wordfix::WordFixes::default(),
    };
    let flat = fixes::load(&config_dir.join("word-fixes.json"));
    move |text: &str| fixes::apply(&personal.apply(text), &flat)
}

fn threads() -> i32 {
    std::env::var("CAW_SPEECH_THREADS").ok().and_then(|s| s.parse().ok()).filter(|&n| n > 0).unwrap_or(2)
}


// ---- the speech worker ----

/// What the worker does, in order. A hold is Start, Audio..., Finish.
enum Job {
    Start,
    Audio { rate: u32, samples: Vec<f32> },
    Finish(mpsc::Sender<Heard>),
}

/// One hold's result.
#[derive(Default)]
struct Heard {
    /// Joined segments, before cleaning.
    raw: String,
    /// The whole hold at 16 kHz, kept only until it is typed (or saved as unsent).
    audio: Vec<f32>,
    /// The loudest 100 ms (RMS) after the first 0.2 s.
    loudest: f32,
    /// One per decoded segment.
    timings: Vec<Timing>,
    error: Option<String>,
}

struct Session {
    resampler: Option<(u32, sherpa_onnx::LinearResampler)>,
    filter: HighPass,
    rec: Segmented,
    audio: Vec<f32>,
}

impl Session {
    fn new() -> Self {
        Self { resampler: None, filter: HighPass::new(), rec: Segmented::default(), audio: Vec::new() }
    }

    fn feed(&mut self, model: Option<&Moonshine>, rate: u32, samples: Vec<f32>) -> Result<()> {
        let mut samples = if rate == SAMPLE_RATE {
            samples
        } else {
            if self.resampler.as_ref().is_none_or(|(r, _)| *r != rate) {
                let resampler = sherpa_onnx::LinearResampler::create(rate as i32, SAMPLE_RATE as i32).context("can't resample the microphone's audio")?;
                self.resampler = Some((rate, resampler));
            }
            self.resampler.as_ref().map(|(_, r)| r.resample(&samples, false)).unwrap_or_default()
        };
        self.filter.process(&mut samples);
        if let Some(model) = model {
            self.rec.feed(model, &samples);
        }
        self.audio.extend_from_slice(&samples);
        Ok(())
    }

    fn finish(mut self, model: Option<&Moonshine>, load_error: Option<&str>, feed_error: Option<String>) -> Heard {
        let raw = model.map(|m| self.rec.finish(m)).unwrap_or_default();
        let error = feed_error.or_else(|| load_error.map(|e| format!("the speech model isn't loaded: {e}")));
        let skip = (SAMPLE_RATE as usize / 5).min(self.audio.len());
        let loudest = self.audio[skip..].chunks(segmenter::CHUNK_SAMPLES).map(segmenter::rms).fold(0.0, f32::max);
        Heard { raw, audio: self.audio, loudest, timings: std::mem::take(&mut self.rec.timings), error }
    }
}

/// Loads the model, then runs jobs one at a time until every sender is gone.
fn worker(model_dir: PathBuf, jobs: mpsc::Receiver<Job>) {
    let started = Instant::now();
    let (model, load_error) = match Moonshine::load(&model_dir, threads()) {
        Ok(m) => {
            log(format!("speech model loaded in {} ms ({})", started.elapsed().as_millis(), model_dir.display()));
            (Some(m), None)
        }
        Err(e) => {
            let why = format!("{e:#}");
            log(format!("speech model did not load: {why}"));
            notify(&format!("The speech model did not load: {why}"));
            (None, Some(why))
        }
    };
    let mut session: Option<(Session, Option<String>)> = None;
    for job in jobs {
        match job {
            Job::Start => session = Some((Session::new(), None)),
            Job::Audio { rate, samples } => {
                if let Some((s, feed_error)) = &mut session {
                    if feed_error.is_none() {
                        if let Err(e) = s.feed(model.as_ref(), rate, samples) {
                            *feed_error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            Job::Finish(reply) => {
                let heard = match session.take() {
                    Some((s, feed_error)) => s.finish(model.as_ref(), load_error.as_deref(), feed_error),
                    None => Heard::default(),
                };
                let _ = reply.send(heard);
            }
        }
    }
}

// ---- the daemon ----

enum Phase {
    Idle,
    Listening { hold: u64, capture: audio::Capture },
    /// Released; the microphone runs on until the tail ends.
    Tail { hold: u64, capture: audio::Capture },
}

/// What the app shows: the pill and the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    /// The microphone is on (held, or in the short tail after release).
    Listening,
    /// Transcribing or typing a finished hold.
    Working,
}

/// Told to the app as they happen.
#[derive(Debug, Clone)]
pub enum Event {
    Activity(Activity),
    /// A hold's text, as typed (without the trailing space). Kept in memory only.
    Heard(String),
    /// A hold's text was typed.
    Typed,
    /// Something went wrong, in words for people.
    Failed(String),
}

type Hook = Box<dyn Fn(Event) + Send + Sync>;

pub struct Daemon {
    config_dir: PathBuf,
    jobs: Mutex<mpsc::Sender<Job>>,
    phase: Mutex<Phase>,
    holds: std::sync::atomic::AtomicU64,
    /// Off (the app's "Dictation" switch): `start` does nothing.
    enabled: AtomicBool,
    /// The microphone's level now, 0..1 (f32 bits), as Caw for Mac measures it: 3 × the peak.
    level: Arc<AtomicU32>,
    /// Holds handed to the worker and not yet typed.
    pending: AtomicUsize,
    hook: Hook,
}

impl Daemon {
    fn send(&self, job: Job) {
        let _ = self.jobs.lock().unwrap().send(job);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn activity(&self) -> Activity {
        match *self.phase.lock().unwrap() {
            Phase::Listening { .. } | Phase::Tail { .. } => Activity::Listening,
            Phase::Idle if self.pending.load(Ordering::Relaxed) > 0 => Activity::Working,
            Phase::Idle => Activity::Idle,
        }
    }

    fn changed(&self) {
        (self.hook)(Event::Activity(self.activity()));
    }

    /// `start`, logging and reporting a failure.
    pub fn start_reporting(self: &Arc<Self>) -> Result<&'static str> {
        self.start().map_err(|e| {
            log(format!("could not start recording: {e:#}"));
            notify(&format!("Could not start recording: {e:#}"));
            (self.hook)(Event::Failed(format!("Could not start recording: {e:#}")));
            e
        })
    }

    /// Starts if idle, stops if listening (the pill's click, `talk toggle`).
    pub fn toggle(self: &Arc<Self>) -> Result<&'static str> {
        match self.status() {
            "listening" => Ok(self.stop()),
            _ => self.start_reporting(),
        }
    }

    fn status(&self) -> &'static str {
        match *self.phase.lock().unwrap() {
            Phase::Idle => "idle",
            Phase::Listening { .. } => "listening",
            Phase::Tail { .. } => "finishing",
        }
    }

    fn start(self: &Arc<Self>) -> Result<&'static str> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok("off");
        }
        let started = self.start_locked()?;
        self.changed();
        Ok(started)
    }

    fn start_locked(self: &Arc<Self>) -> Result<&'static str> {
        let mut phase = self.phase.lock().unwrap();
        match std::mem::replace(&mut *phase, Phase::Idle) {
            Phase::Idle => {}
            // Pressed again within the tail: the microphone is still running, carry on.
            Phase::Tail { hold, capture } | Phase::Listening { hold, capture } => {
                *phase = Phase::Listening { hold, capture };
                return Ok("listening");
            }
        }
        let hold = self.holds.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.send(Job::Start);
        let jobs = self.jobs.lock().unwrap().clone();
        let level = self.level.clone();
        let capture = match audio::start(move |rate, samples| {
            let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            level.store((peak * 3.0).min(1.0).to_bits(), Ordering::Relaxed);
            let _ = jobs.send(Job::Audio { rate, samples });
        }) {
            Ok(c) => c,
            Err(e) => {
                // Close the empty hold so the worker isn't left waiting.
                let (tx, _rx) = mpsc::channel();
                self.send(Job::Finish(tx));
                return Err(e);
            }
        };
        if capture.rate != SAMPLE_RATE {
            log(format!("the microphone records at {} Hz; resampling to {SAMPLE_RATE} Hz", capture.rate));
        }
        *phase = Phase::Listening { hold, capture };
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(MAX_HOLD).await;
            if me.end(hold, true) {
                log("recording stopped after 5 minutes");
                notify("Recording stopped after 5 minutes. Finishing the transcript…");
            }
        });
        Ok("listening")
    }

    pub fn stop(self: &Arc<Self>) -> &'static str {
        let mut phase = self.phase.lock().unwrap();
        let Phase::Listening { .. } = &*phase else { return "idle" };
        let Phase::Listening { hold, capture } = std::mem::replace(&mut *phase, Phase::Idle) else { unreachable!() };
        *phase = Phase::Tail { hold, capture };
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(TAIL).await;
            me.end(hold, false);
        });
        "finishing"
    }

    /// Ends hold `hold` if it is still recording (still held, too, if `even_held`):
    /// stops the microphone and hands the audio to the worker. False if it had already ended.
    fn end(self: &Arc<Self>, hold: u64, even_held: bool) -> bool {
        let mut phase = self.phase.lock().unwrap();
        let current = match &*phase {
            Phase::Tail { hold: h, .. } => *h == hold,
            Phase::Listening { hold: h, .. } => even_held && *h == hold,
            Phase::Idle => false,
        };
        if !current {
            return false;
        }
        let (Phase::Tail { capture, .. } | Phase::Listening { capture, .. }) = std::mem::replace(&mut *phase, Phase::Idle) else { unreachable!() };
        // Under the lock, so the next hold's Start can't overtake this Finish.
        capture.stop();
        self.level.store(0, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.send(Job::Finish(tx));
        self.pending.fetch_add(1, Ordering::Relaxed);
        drop(phase);
        self.changed();
        let me = self.clone();
        tokio::spawn(async move {
            let heard = tokio::task::spawn_blocking(move || rx.recv().unwrap_or_default()).await.unwrap_or_default();
            me.deliver(heard).await;
            me.pending.fetch_sub(1, Ordering::Relaxed);
            me.changed();
        });
        true
    }

    async fn deliver(&self, heard: Heard) {
        let fix = word_fixes(&self.config_dir);
        let text = cleaner::clean(&heard.raw, fix);
        if let Some(why) = &heard.error {
            return self.unsent(&heard, &text, why);
        }
        if text.is_empty() {
            if heard.loudest >= SPEECH_LEVEL {
                return self.unsent(&heard, &text, "no words were recognised");
            }
            return log(format!("nothing heard (loudest {:.3})", heard.loudest));
        }
        // As the phone sends it: a trailing space, so the next dictation follows on. Never a Return.
        let line: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        (self.hook)(Event::Heard(line.clone()));
        let _turn = typer::TURN.lock().await;
        match typer::type_text(&(line + " ")).await {
            Ok(()) => (self.hook)(Event::Typed),
            Err(e) => self.unsent(&heard, &text, &format!("typing failed: {e:#}")),
        }
    }

    fn unsent(&self, heard: &Heard, text: &str, why: &str) {
        let why = upper_first(why);
        (self.hook)(Event::Failed(why.clone()));
        match save_unsent(&heard.audio, text) {
            Ok(path) => {
                log(format!("{why}; saved the recording as {}", path.display()));
                notify(&format!("{why}. The recording{} is saved in {}", if text.is_empty() { "" } else { " and text" }, path.parent().unwrap_or(&path).display()));
            }
            Err(e) => {
                log(format!("{why}; could not save the recording either: {e:#}"));
                notify(&format!("{why}, and the recording could not be saved: {e:#}"));
            }
        }
    }
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Saves a hold as unsent/<time>.wav plus <time>.txt with any text. Returns the .wav path.
fn save_unsent(audio: &[f32], text: &str) -> Result<PathBuf> {
    let dir = data_dir().join("unsent");
    std::fs::create_dir_all(&dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let stem = local_stamp();
    let wav = dir.join(format!("{stem}.wav"));
    if !audio.is_empty() && !sherpa_onnx::write(&wav.to_string_lossy(), audio, SAMPLE_RATE as i32) {
        bail!("could not write {}", wav.display());
    }
    if !text.is_empty() {
        std::fs::write(dir.join(format!("{stem}.txt")), format!("{text}\n"))?;
    }
    Ok(wav)
}

/// Local time as 2026-01-31-142501, for file names.
fn local_stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return now.to_string();
    }
    format!("{:04}-{:02}-{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn log(msg: impl AsRef<str>) {
    eprintln!("caw-desktop: {}", msg.as_ref());
}

static QUIET: AtomicBool = AtomicBool::new(false);

/// The app shows errors on the floating Crumb instead of as notifications while he is shown.
#[cfg_attr(not(feature = "app-base"), allow(dead_code))]
pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

/// A desktop notification, for errors only. Quietly does nothing without notify-send.
fn notify(msg: &str) {
    if cfg!(test) || QUIET.load(Ordering::Relaxed) {
        return;
    }
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=Caw", "--icon=audio-input-microphone", "Caw dictation", msg])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| std::thread::spawn(move || child.wait()));
}

/// `caw-desktop dictate`: the daemon.
pub async fn run(config_dir: PathBuf) -> Result<()> {
    run_with(config_dir, Options { enabled: true, hook: Box::new(|_| {}), ready: None, signals: true }).await
}

/// How the app runs dictation.
pub struct Options {
    /// The app's "Dictation" switch at start.
    pub enabled: bool,
    pub hook: Box<dyn Fn(Event) + Send + Sync>,
    /// Gets the daemon once it is listening on the talk socket.
    pub ready: Option<tokio::sync::oneshot::Sender<Arc<Daemon>>>,
    /// Stop on Ctrl+C and SIGTERM (the command line; the app handles them itself).
    pub signals: bool,
}

pub async fn run_with(config_dir: PathBuf, opts: Options) -> Result<()> {
    let path = socket_path();
    if UnixStream::connect(&path).await.is_ok() {
        bail!("dictation is already running ({})", path.display());
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("listening on {}", path.display()))?;

    #[cfg(target_os = "linux")]
    {
        let input = crate::linux::init(&config_dir).await;
        log(format!("typing with {}", input.describe()));
    }
    let settings = Settings::load(&config_dir);
    let (tx, rx) = mpsc::channel();
    let model_dir = settings.model_dir();
    std::thread::Builder::new().name("caw-speech".into()).spawn(move || worker(model_dir, rx))?;
    let daemon = Arc::new(Daemon {
        config_dir,
        jobs: Mutex::new(tx),
        phase: Mutex::new(Phase::Idle),
        holds: Default::default(),
        enabled: AtomicBool::new(opts.enabled),
        level: Default::default(),
        pending: Default::default(),
        hook: opts.hook,
    });
    log(format!("dictation ready: {} ({})", binding::startup_hint(&settings.talk_key()), path.display()));
    if let Some(ready) = opts.ready {
        let _ = ready.send(daemon.clone());
    }

    // Only on the command line: in the app, the signal handlers are the app's (it quits as a whole).
    let mut term = if opts.signals { Some(tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?) } else { None };
    loop {
        let signalled = async {
            match term.as_mut() {
                Some(term) => {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = term.recv() => {}
                    }
                }
                None => std::future::pending::<()>().await,
            }
        };
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((s, _)) => s,
                Err(e) => { log(format!("control socket: {e}")); continue; }
            },
            () = signalled => break,
        };
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            if BufReader::new(read).read_line(&mut line).await.is_err() {
                return;
            }
            let reply = match line.trim() {
                "start" => daemon.start_reporting(),
                "stop" => Ok(daemon.stop()),
                "toggle" => daemon.toggle(),
                "status" => Ok(daemon.status()),
                other => Err(anyhow::anyhow!("unknown command {other:?}")),
            };
            let reply = match reply {
                Ok(s) => s.to_string(),
                Err(e) => format!("error: {e:#}"),
            };
            let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
        });
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// `caw-desktop talk start|stop|toggle|status|bindings [key]`.
pub async fn talk(config_dir: PathBuf, args: &[String]) -> Result<()> {
    let command = match args.first().map(String::as_str) {
        Some(c @ ("start" | "stop" | "toggle" | "status")) => c,
        Some("bindings") => {
            print_bindings(&config_dir, args.get(1).map(String::as_str));
            return Ok(());
        }
        _ => bail!("usage: caw-desktop talk start|stop|toggle|status|bindings [key]"),
    };
    let mut stream = match UnixStream::connect(socket_path()).await {
        Ok(s) => s,
        Err(_) => {
            if command != "stop" && command != "status" {
                notify("Dictation isn't running. Start it with: caw-desktop dictate");
            }
            bail!("dictation isn't running (start it with: caw-desktop dictate)");
        }
    };
    stream.write_all(format!("{command}\n").as_bytes()).await?;
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(5), BufReader::new(stream).read_line(&mut reply)).await.context("no answer from dictation")??;
    let reply = reply.trim();
    match reply.strip_prefix("error: ") {
        Some(e) => bail!("{e}"),
        None => {
            println!("{reply}");
            Ok(())
        }
    }
}

/// Hyprland lines for `key` (ctrl_r, alt_r, fn, a Hyprland key name such as
/// Alt_R or XF86Fn, or code:<n>), else for the talk key setting: press starts,
/// release stops. Lua (`o.bind` or `hl.bind`) or hyprland.conf, as the config uses.
fn print_bindings(config_dir: &Path, key: Option<&str>) {
    let talk_key = key.map(String::from).unwrap_or_else(|| Settings::load(config_dir).talk_key());
    let dir = binding::hypr_dir();
    let style = binding::style(&dir);
    if let Some(bound) = binding::find(&dir) {
        println!("{} Bound now: {} (in {})", if style == binding::Style::Conf { "#" } else { "--" }, bound.keys, bound.file.display());
    }
    println!("{} Hyprland: add to {} (replacing any older Caw lines).", if style == binding::Style::Conf { "#" } else { "--" }, binding::target_file(&dir, style).display());
    for line in binding::lines(style, &talk_key, &binding::exe()) {
        println!("{line}");
    }
}

/// `caw-desktop dictate --file x.wav [--live] [--type]`: transcribes a file the
/// same way a hold is transcribed, and prints timings. `--live` feeds it at
/// real-time speed, as a microphone would; `--type` types the result (honours CAW_DRY_RUN).
pub async fn transcribe_file(config_dir: PathBuf, path: &Path, live: bool, type_it: bool) -> Result<()> {
    let wave = sherpa_onnx::Wave::read(&path.to_string_lossy()).with_context(|| format!("reading {} (a 16-bit mono WAV file)", path.display()))?;
    let rate = wave.sample_rate() as u32;
    let samples = wave.samples().to_vec();
    let secs = samples.len() as f32 / rate as f32;
    let settings = Settings::load(&config_dir);
    let started = Instant::now();
    let model = Moonshine::load(&settings.model_dir(), threads())?;
    let load_ms = started.elapsed().as_millis();
    println!("model loaded in {load_ms} ms ({} threads); memory after loading: {}", threads(), memory());

    let mut session = Session::new();
    let chunk = (rate / 10) as usize;
    let fed_at = Instant::now();
    for (i, piece) in samples.chunks(chunk).enumerate() {
        if live {
            let due = Duration::from_secs_f32(i as f32 * 0.1);
            if let Some(wait) = due.checked_sub(fed_at.elapsed()) {
                tokio::time::sleep(wait).await;
            }
        }
        session.feed(Some(&model), rate, piece.to_vec())?;
    }
    let released = Instant::now();
    let heard = session.finish(Some(&model), None, None);
    let after_release_ms = released.elapsed().as_millis();
    let total_ms = fed_at.elapsed().as_millis();

    let fix = word_fixes(&config_dir);
    let text = cleaner::clean(&heard.raw, fix);
    println!("audio: {secs:.1} s, {} segment(s), loudest 100 ms {:.3}", heard.timings.len(), heard.loudest);
    for (i, t) in heard.timings.iter().enumerate() {
        println!("  segment {}: {:.1} s of audio decoded in {} ms", i + 1, t.audio_secs, t.decode_ms);
    }
    println!("decoding: {total_ms} ms in all ({:.2} x real time{}), {after_release_ms} ms after the audio ended", total_ms as f32 / 1000.0 / secs, if live { ", fed live" } else { "" });
    println!("memory: {}", memory());
    println!("raw:   {}", heard.raw);
    println!("typed: {text}");
    if type_it {
        typer::type_text(&format!("{text} ")).await?;
    }
    Ok(())
}

/// Resident memory now and at its peak, from /proc.
fn memory() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| status.lines().find(|l| l.starts_with(name)).map(|l| l[name.len()..].trim().to_string()).unwrap_or_else(|| "?".into());
    format!("{} resident, {} at peak", field("VmRSS:"), field("VmHWM:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talk_key_default_depends_on_the_keyboard() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.talk_key, None);
        assert!(["ctrl_r", "fn"].contains(&s.talk_key().as_str()));
        let s: Settings = serde_json::from_str(r#"{"talk_key": "alt_r"}"#).unwrap();
        assert_eq!(s.talk_key(), "alt_r");
        assert_eq!(serde_json::to_string(&Settings::default()).unwrap(), "{}");
    }

    #[test]
    fn personal_fixes_run_before_the_phone_list() {
        let dir = std::env::temp_dir().join(format!("caw-fixes-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("word-fixes.personal.json"), r#"{"version":1,"fixes":[{"from":"akme|ackme","to":"acme","capitalization":"always"}]}"#).unwrap();
        std::fs::write(dir.join("word-fixes.json"), r#"{"acme": "ACME Inc"}"#).unwrap();
        let fix = word_fixes(&dir);
        assert_eq!(fix("ask akme now"), "ask ACME Inc now");
        std::fs::write(dir.join("word-fixes.personal.json"), "not json").unwrap();
        assert_eq!(word_fixes(&dir)("ask akme now"), "ask akme now");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(word_fixes(&dir)("ask akme now"), "ask akme now");
    }

    #[test]
    fn stamp_is_file_name_safe() {
        let s = local_stamp();
        assert!(s.chars().all(|c| c.is_ascii_digit() || c == '-'), "{s}");
    }
}
