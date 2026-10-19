//! How to type and move the mouse on this Linux desktop, chosen once at startup:
//! - wlroots compositors (Hyprland, Sway): their virtual keyboard and pointer;
//! - other Wayland desktops (GNOME, KDE Plasma): the remote-desktop portal;
//! - X11 desktops (Cinnamon, XFCE, MATE): the XTEST extension.
//!
//! `CAW_INPUT=wlr|portal|x11` overrides the choice.

pub mod evdev;
pub mod keysym;
mod portal;
pub mod vkbd;
mod x11;

use anyhow::{bail, Result};
use std::path::Path;
use std::sync::OnceLock;

pub use portal::Portal;
pub use x11::X11;

pub enum Input {
    Wlr,
    Portal(Portal),
    X11(Box<X11>),
    Unavailable(String),
}

static INPUT: OnceLock<Input> = OnceLock::new();

pub fn get() -> &'static Input {
    INPUT.get_or_init(|| Input::Unavailable("input was not set up".into()))
}

pub async fn init(config_dir: &Path) -> &'static Input {
    // Once per process (the app runs the receiver and dictation together).
    if let Some(input) = INPUT.get() {
        return input;
    }
    let choice = std::env::var("CAW_INPUT").unwrap_or_default();
    let input = pick(&choice, config_dir).await.unwrap_or_else(|e| Input::Unavailable(format!("{e:#}")));
    if let Input::Wlr = input {
        // Read now, before wtype types anything and its keymap becomes the seat's.
        let _ = vkbd::remember_seat_keymap();
    }
    let _ = INPUT.set(input);
    get()
}

async fn pick(choice: &str, config_dir: &Path) -> Result<Input> {
    let portal = || async {
        eprintln!("caw-desktop: asking the desktop for permission to control the keyboard and mouse");
        Ok(Input::Portal(Portal::open(&config_dir.join("portal-token")).await?))
    };
    match choice {
        "wlr" => Ok(Input::Wlr),
        "portal" => portal().await,
        "x11" => Ok(Input::X11(Box::new(X11::open()?))),
        "" if std::env::var_os("WAYLAND_DISPLAY").is_some() => {
            if crate::pointer::wayland::has_virtual_input() {
                Ok(Input::Wlr)
            } else {
                portal().await
            }
        }
        "" if std::env::var_os("DISPLAY").is_some() => Ok(Input::X11(Box::new(X11::open()?))),
        "" => bail!("no graphical session found (neither WAYLAND_DISPLAY nor DISPLAY is set)"),
        other => bail!("unknown CAW_INPUT {other:?}; use wlr, portal or x11"),
    }
}

impl Input {
    pub fn describe(&self) -> String {
        match self {
            Input::Wlr => "Wayland virtual keyboard and pointer".into(),
            Input::Portal(_) => "the desktop's remote-desktop portal".into(),
            Input::X11(_) => "X11 (XTEST)".into(),
            Input::Unavailable(why) => format!("none ({why})"),
        }
    }
}
