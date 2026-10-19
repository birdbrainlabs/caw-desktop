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

mod audio;
mod recognizer;

use crate::{fixes, typer};
use anyhow::{bail, Context, Result};
use caw_desktop::speech::{cleaner, filter::HighPass, segmenter};
use recognizer::{Moonshine, Segmented, Timing, SAMPLE_RATE};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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
    /// The hold-to-talk key: "ctrl_r" (Right Ctrl, the default), "alt_r",
    /// "super_r", a keysym name such as "F13", or "code:<n>" for a raw key code.
    /// Caw doesn't watch the key itself; the desktop's shortcut runs
    /// `caw-desktop talk start` / `talk stop` (see `caw-desktop talk bindings`).
    pub talk_key: String,
    /// The folder with the Moonshine model files. Default:
    /// ~/.local/share/caw-desktop/models/moonshine-base-en. CAW_MODEL_DIR overrides it.
    pub model_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { talk_key: "ctrl_r".into(), model_dir: None }
    }
}

impl Settings {
    pub fn load(config_dir: &Path) -> Self {
        std::fs::read(config_dir.join("dictation.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn model_dir(&self) -> PathBuf {
        if let Some(dir) = std::env::var_os("CAW_MODEL_DIR") {
            return dir.into();
        }
        self.model_dir.clone().unwrap_or_else(|| data_dir().join("models").join("moonshine-base-en"))
    }
}

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("caw-desktop")
}

fn socket_path() -> PathBuf {
    match std::env::var_os("CAW_DICTATE_SOCKET") {
        Some(p) => p.into(),
        None => dirs::runtime_dir().unwrap_or_else(std::env::temp_dir).join("caw-dictate.sock"),
    }
}

fn threads() -> i32 {
    std::env::var("CAW_SPEECH_THREADS").ok().and_then(|s| s.parse().ok()).filter(|&n| n > 0).unwrap_or(2)
}

/// The Hyprland key name for a talk key, and the modifier that is down when it is released.
fn hyprland_key(talk_key: &str) -> (String, &'static str) {
    match talk_key.trim().to_lowercase().as_str() {
        "ctrl_r" | "right_ctrl" => ("Control_R".into(), "CTRL"),
        "alt_r" | "right_alt" | "right_option" => ("Alt_R".into(), "ALT"),
        "super_r" | "right_super" => ("Super_R".into(), "SUPER"),
        _ => (talk_key.trim().to_string(), ""),
    }
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

struct Daemon {
    config_dir: PathBuf,
    jobs: Mutex<mpsc::Sender<Job>>,
    phase: Mutex<Phase>,
    holds: std::sync::atomic::AtomicU64,
    /// One hold types at a time, in the order they finished.
    typing: tokio::sync::Mutex<()>,
}

impl Daemon {
    fn send(&self, job: Job) {
        let _ = self.jobs.lock().unwrap().send(job);
    }

    fn status(&self) -> &'static str {
        match *self.phase.lock().unwrap() {
            Phase::Idle => "idle",
            Phase::Listening { .. } => "listening",
            Phase::Tail { .. } => "finishing",
        }
    }

    fn start(self: &Arc<Self>) -> Result<&'static str> {
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
        let capture = match audio::start(move |rate, samples| {
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

    fn stop(self: &Arc<Self>) -> &'static str {
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
        let (tx, rx) = mpsc::channel();
        self.send(Job::Finish(tx));
        drop(phase);
        let me = self.clone();
        tokio::spawn(async move {
            let heard = tokio::task::spawn_blocking(move || rx.recv().unwrap_or_default()).await.unwrap_or_default();
            me.deliver(heard).await;
        });
        true
    }

    async fn deliver(&self, heard: Heard) {
        let fixes = fixes::load(&self.config_dir.join("word-fixes.json"));
        let text = cleaner::clean(&heard.raw, |t| fixes::apply(t, &fixes));
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
        let typed: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>() + " ";
        let _turn = self.typing.lock().await;
        if let Err(e) = typer::type_text(&typed).await {
            self.unsent(&heard, &text, &format!("typing failed: {e:#}"));
        }
    }

    fn unsent(&self, heard: &Heard, text: &str, why: &str) {
        let why = upper_first(why);
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

/// A desktop notification, for errors only. Quietly does nothing without notify-send.
fn notify(msg: &str) {
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
    let daemon = Arc::new(Daemon { config_dir, jobs: Mutex::new(tx), phase: Mutex::new(Phase::Idle), holds: Default::default(), typing: Default::default() });
    let (key, _) = hyprland_key(&settings.talk_key);
    log(format!("dictation ready: hold {key} to talk ({})", path.display()));

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((s, _)) => s,
                Err(e) => { log(format!("control socket: {e}")); continue; }
            },
            _ = tokio::signal::ctrl_c() => break,
            _ = term.recv() => break,
        };
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            if BufReader::new(read).read_line(&mut line).await.is_err() {
                return;
            }
            let reply = match line.trim() {
                "start" => daemon.start().map_err(|e| {
                    log(format!("could not start recording: {e:#}"));
                    notify(&format!("Could not start recording: {e:#}"));
                    e
                }),
                "stop" => Ok(daemon.stop()),
                "toggle" => match daemon.status() {
                    "listening" => Ok(daemon.stop()),
                    _ => daemon.start(),
                },
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

/// `caw-desktop talk start|stop|toggle|status|bindings`.
pub async fn talk(config_dir: PathBuf, command: Option<&str>) -> Result<()> {
    let command = match command {
        Some(c @ ("start" | "stop" | "toggle" | "status")) => c,
        Some("bindings") => {
            print_bindings(&config_dir);
            return Ok(());
        }
        _ => bail!("usage: caw-desktop talk start|stop|toggle|status|bindings"),
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

fn print_bindings(config_dir: &Path) {
    let settings = Settings::load(config_dir);
    let (key, release_mod) = hyprland_key(&settings.talk_key);
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "caw-desktop".into());
    println!("# Caw hold-to-talk ({}). Hyprland: add to your bindings file.", settings.talk_key);
    println!("bind = , {key}, exec, {exe} talk start");
    println!("bindr = {release_mod}, {key}, exec, {exe} talk stop");
    if !release_mod.is_empty() {
        // Some setups report the modifier on release, some don't; a second stop does nothing.
        println!("bindr = , {key}, exec, {exe} talk stop");
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

    let fixes = fixes::load(&config_dir.join("word-fixes.json"));
    let text = cleaner::clean(&heard.raw, |t| fixes::apply(t, &fixes));
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
    fn talk_key_defaults_to_right_ctrl() {
        assert_eq!(Settings::default().talk_key, "ctrl_r");
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.talk_key, "ctrl_r");
        assert_eq!(hyprland_key("ctrl_r"), ("Control_R".into(), "CTRL"));
        assert_eq!(hyprland_key("alt_r"), ("Alt_R".into(), "ALT"));
        assert_eq!(hyprland_key("code:472"), ("code:472".into(), ""));
    }

    #[test]
    fn stamp_is_file_name_safe() {
        let s = local_stamp();
        assert!(s.chars().all(|c| c.is_ascii_digit() || c == '-'), "{s}");
    }
}
