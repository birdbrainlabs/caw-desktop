//! The hold-to-talk key through the desktop's GlobalShortcuts portal (GNOME,
//! KDE Plasma): the desktop owns the key, asks the person once whether Caw
//! may have it, and tells Caw when it goes down and when it comes up. Caw
//! still never reads the keyboard itself. Hyprland keeps its own bindings
//! (dictate::binding), so this isn't used there.
//!
//! Everything the desktop answers is logged, since what each desktop allows
//! (a lone modifier such as Right Ctrl, for one) differs.
//!
//! GNOME tells Caw only that the key went down, never that it came up, so
//! there the key starts talking and the next press stops it
//! ([`press_to_toggle`]). Other desktops keep hold-to-talk.

use crate::dictate::{binding, Daemon};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::ResponseError;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

/// The shortcut's id with the desktop.
pub const ID: &str = "talk";

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Asking the desktop (it may be showing its dialog).
    Asking,
    /// Bound. The desktop's own name for the key, if it gave one.
    Bound(String),
    /// The dialog was answered without giving Caw a key.
    Declined,
    /// This desktop can't do it, in words for people.
    Unavailable(String),
}

/// GNOME (Ubuntu's desktop among them): the talk key is pressed once to
/// start and again to stop, since GNOME sends no key-up. Hyprland and other
/// desktops hold it.
pub fn press_to_toggle() -> bool {
    !binding::on_hyprland() && on_gnome()
}

/// GNOME, whatever else XDG_CURRENT_DESKTOP lists ("ubuntu:GNOME").
pub fn on_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("gnome")))
}

/// The trigger to suggest for a talk key setting, as the XDG shortcuts
/// specification writes keys (xkb key names): Control_R, Alt_R, XF86Fn, F13.
pub fn trigger(talk_key: &str) -> String {
    binding::hyprland_key(talk_key).0
}

fn log(msg: impl AsRef<str>) {
    super::log(format!("talk key: {}", msg.as_ref()))
}

/// Binds the talk key and runs dictation from it until `rebind` closes. Each
/// talk key sent on `rebind` (a change in Settings) asks the desktop again.
pub async fn run(daemon: Arc<Daemon>, mut talk_key: String, mut rebind: UnboundedReceiver<String>, report: impl Fn(Status) + Send + Sync + 'static) {
    loop {
        match session(&daemon, &talk_key, &mut rebind, &report).await {
            Ok(Some(key)) => talk_key = key,
            Ok(None) => return,
            Err(status) => {
                log(format!("{status:?}"));
                report(status);
                // Try again only when Settings asks for a (new) key.
                match rebind.recv().await {
                    Some(key) => talk_key = key,
                    None => return,
                }
            }
        }
    }
}

/// One session: Ok(Some(key)) when asked to bind another key, Ok(None) when the app is closing.
async fn session(daemon: &Arc<Daemon>, talk_key: &str, rebind: &mut UnboundedReceiver<String>, report: &(impl Fn(Status) + Send + Sync)) -> Result<Option<String>, Status> {
    let unavailable = |what: &str, e: ashpd::Error| Status::Unavailable(format!("{what}: {e}"));
    let portal = match GlobalShortcuts::new().await {
        Ok(p) => p,
        Err(ashpd::Error::PortalNotFound(_)) => return Err(Status::Unavailable("this desktop has no global-shortcuts portal".into())),
        Err(e) => return Err(unavailable("no global-shortcuts portal", e)),
    };
    log(format!("the desktop's global-shortcuts portal is version {}", portal.version()));
    let session = portal.create_session(Default::default()).await.map_err(|e| unavailable("the desktop refused a shortcuts session", e))?;
    let mut activated = portal.receive_activated().await.map_err(|e| unavailable("shortcut signals", e))?;
    let mut deactivated = portal.receive_deactivated().await.map_err(|e| unavailable("shortcut signals", e))?;
    let mut changed = portal.receive_shortcuts_changed().await.map_err(|e| unavailable("shortcut signals", e))?;

    let trigger = trigger(talk_key);
    log(format!("asking the desktop for {trigger} (hold to talk)"));
    report(Status::Asking);
    let shortcut = NewShortcut::new(ID, "Hold to talk (Caw dictation)").preferred_trigger(Some(trigger.as_str()));
    let shortcuts = [shortcut];
    let request = portal.bind_shortcuts(&session, &shortcuts, None, Default::default());
    // The desktop may show a dialog and wait for the person; Settings can ask for another key meanwhile.
    let request = tokio::select! {
        r = request => r,
        key = rebind.recv() => {
            let _ = session.close().await;
            return Ok(key);
        }
    };
    let bound = match request.map(|r| r.response()) {
        Ok(Ok(bound)) => bound,
        Ok(Err(ashpd::Error::Response(ResponseError::Cancelled))) | Err(ashpd::Error::Response(ResponseError::Cancelled)) => {
            let _ = session.close().await;
            return Err(Status::Declined);
        }
        Ok(Err(e)) | Err(e) => {
            let _ = session.close().await;
            return Err(unavailable("the desktop didn't bind the key", e));
        }
    };
    let describe = |list: &[ashpd::desktop::global_shortcuts::Shortcut]| list.iter().find(|s| s.id() == ID).map(|s| s.trigger_description().to_string());
    log(format!("the desktop answered: {:?}", bound.shortcuts().iter().map(|s| (s.id(), s.trigger_description())).collect::<Vec<_>>()));
    let Some(description) = describe(bound.shortcuts()) else {
        let _ = session.close().await;
        return Err(Status::Declined);
    };
    report(Status::Bound(description));
    let toggle = press_to_toggle();
    if toggle {
        log("this desktop sends no key-up: a press starts talking, the next press stops");
    }

    loop {
        tokio::select! {
            Some(a) = activated.next() => {
                if a.shortcut_id() == ID {
                    log("pressed");
                    if toggle {
                        if let Ok(now) = daemon.toggle() {
                            log(format!("now {now}"));
                        }
                    } else {
                        let _ = daemon.start_reporting();
                    }
                }
            }
            Some(d) = deactivated.next() => {
                if d.shortcut_id() == ID {
                    if toggle {
                        // The next press stops, not this release.
                        log("released (ignored: the next press stops)");
                    } else {
                        log("released");
                        daemon.stop();
                    }
                }
            }
            Some(c) = changed.next() => {
                log(format!("the desktop changed it: {:?}", c.shortcuts().iter().map(|s| (s.id(), s.trigger_description())).collect::<Vec<_>>()));
                if let Some(description) = describe(c.shortcuts()) {
                    report(Status::Bound(description));
                }
            }
            key = rebind.recv() => {
                let _ = session.close().await;
                return Ok(key);
            }
        }
    }
}
