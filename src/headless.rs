//! `caw-desktop serve --headless`: the receiver run by another app (Caw for
//! Mac bundles it as a helper), with no menu bar icon, browser page or
//! permission dialog of its own.
//!
//! - **stdout** carries only status, one JSON object per line, each with an
//!   `"event"` field (see `Event`). Logs stay on stderr.
//! - **stdin** takes commands, one JSON object per line, each with a `"cmd"`
//!   field (see `Command`).
//! - When stdin closes (the app quit, or crashed), the receiver stops.
//! - Exit status: 0 after stdin closed, 3 when another receiver already has the
//!   port (starting again won't help until it quits), 1 for anything else.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Exit status when another receiver is running or has the port.
pub const EXIT_IN_USE: i32 = 3;

static ON: AtomicBool = AtomicBool::new(false);

/// Another listener for the same events, in this process (the desktop app).
type Hook = Box<dyn Fn(&Event) + Send + Sync>;
static HOOK: std::sync::OnceLock<Hook> = std::sync::OnceLock::new();

/// Also hands every event to `hook`, headless or not. Set once.
#[cfg_attr(not(all(target_os = "linux", feature = "app-base")), allow(dead_code))]
pub fn set_hook(hook: Hook) {
    let _ = HOOK.set(hook);
}

pub fn enable() {
    ON.store(true, Ordering::Relaxed);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Phone {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PairedPhone {
    pub id: String,
    pub name: String,
    /// Unix time in seconds.
    pub paired_at: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The receiver is up. `accessibility`: whether macOS lets this process
    /// (that is, the app that started it) type and move the mouse.
    Listening { port: u16, name: String, id: String, version: String, accessibility: bool },
    /// The receiver could not start or had to stop. `reason`: `already_running`
    /// (another Caw Desktop answers on this Mac), `port_in_use`, or `failed`.
    Stopped { reason: String, message: String },
    /// Every paired phone; sent at start and whenever the list changes.
    Devices { devices: Vec<PairedPhone> },
    Connected { id: String, name: String },
    Disconnected { id: String, name: String },
    /// A pairing code is open. `code` is what the URI carries, `display` is
    /// for people (`XXXX-XXXX-XXXX-XXXX`), `expires_in` is in seconds.
    Pairing { code: String, display: String, uri: String, expires_in: u64 },
    /// The pairing code `code` is closed. `result`: `paired`, `expired`,
    /// `replaced` (a newer code took over), `cancelled` or `failed`.
    PairEnded { code: String, result: String, message: String },
    /// A command could not be carried out.
    Error { message: String },
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Open a pairing code (replacing any open one).
    Pair,
    /// Close the open pairing code, if any.
    CancelPair,
    /// Forget a paired phone by its id, and drop its connection.
    Remove { id: String },
    /// Send the `devices` event again.
    Devices,
}

/// Writes one event line to stdout, in headless mode only (and hands it to the hook).
pub fn emit(event: &Event) {
    if let Some(hook) = HOOK.get() {
        hook(event);
    }
    if !on() {
        return;
    }
    let Ok(line) = serde_json::to_string(event) else { return };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// What a pairing window's free-text outcome means, for `PairEnded`.
pub fn pair_result(outcome: &str) -> &'static str {
    if outcome.starts_with("paired") {
        "paired"
    } else if outcome == "expired" {
        "expired"
    } else if outcome.starts_with("replaced") {
        "replaced"
    } else if outcome == "cancelled" {
        "cancelled"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_lines() {
        let e = Event::Connected { id: "ab12".into(), name: "Test \"Phone\"".into() };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"event":"connected","id":"ab12","name":"Test \"Phone\""}"#);
        let e = Event::PairEnded { code: "C".into(), result: "paired".into(), message: "paired with x".into() };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"event":"pair_ended","code":"C","result":"paired","message":"paired with x"}"#);
    }

    #[test]
    fn commands_parse() {
        assert_eq!(serde_json::from_str::<Command>(r#"{"cmd":"pair"}"#).unwrap(), Command::Pair);
        assert_eq!(serde_json::from_str::<Command>(r#"{"cmd":"cancel_pair"}"#).unwrap(), Command::CancelPair);
        assert_eq!(serde_json::from_str::<Command>(r#"{"cmd":"remove","id":"ab"}"#).unwrap(), Command::Remove { id: "ab".into() });
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"format_disk"}"#).is_err());
    }

    #[test]
    fn outcomes_map_to_results() {
        assert_eq!(pair_result("paired with Test Phone"), "paired");
        assert_eq!(pair_result("expired"), "expired");
        assert_eq!(pair_result("replaced by a new pairing code"), "replaced");
        assert_eq!(pair_result("cancelled"), "cancelled");
        assert_eq!(pair_result("too many wrong codes"), "failed");
    }
}
