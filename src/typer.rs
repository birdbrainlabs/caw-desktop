//! Types into the focused window. Linux uses whichever method `linux` picked
//! for this desktop; Windows uses SendInput with Unicode key events. Both send
//! characters rather than key positions, so any keyboard layout works.

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Key {
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

impl Key {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "backspace" => Self::Backspace,
            "delete" => Self::Delete,
            "enter" => Self::Enter,
            "tab" => Self::Tab,
            "escape" => Self::Escape,
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            "home" => Self::Home,
            "end" => Self::End,
            other => bail!("unknown key {other:?}"),
        })
    }
}

/// CAW_DRY_RUN=1: log instead of typing (for tests).
fn dry_run() -> bool {
    std::env::var_os("CAW_DRY_RUN").is_some()
}

pub async fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if dry_run() {
        eprintln!("caw-desktop: [dry run] type {text:?}");
        return Ok(());
    }
    backend::type_text(text).await
}

pub async fn press_key(key: &str, count: u32) -> Result<()> {
    let key = Key::parse(key)?;
    let count = count.clamp(1, 200);
    if dry_run() {
        eprintln!("caw-desktop: [dry run] key {key:?} x{count}");
        return Ok(());
    }
    backend::press_key(key, count).await
}

#[cfg(target_os = "linux")]
mod backend {
    use super::Key;
    use crate::linux::{self, keysym, Input};
    use anyhow::{bail, Context, Result};
    use tokio::process::Command;

    pub async fn type_text(text: &str) -> Result<()> {
        match linux::get() {
            Input::Wlr => wtype(&["--", text]).await,
            input => tap(input, keysym::for_text(text)).await,
        }
    }

    pub async fn press_key(key: Key, count: u32) -> Result<()> {
        match linux::get() {
            Input::Wlr => {
                let name = match key {
                    Key::Backspace => "BackSpace",
                    Key::Delete => "Delete",
                    Key::Enter => "Return",
                    Key::Tab => "Tab",
                    Key::Escape => "Escape",
                    Key::Left => "Left",
                    Key::Right => "Right",
                    Key::Up => "Up",
                    Key::Down => "Down",
                    Key::Home => "Home",
                    Key::End => "End",
                };
                let args: Vec<&str> = std::iter::repeat_n(["-k", name], count as usize).flatten().collect();
                wtype(&args).await
            }
            input => tap(input, vec![keysym::for_key(key); count as usize]).await,
        }
    }

    async fn tap(input: &'static Input, keysyms: Vec<u32>) -> Result<()> {
        match input {
            Input::Portal(portal) => portal.tap_keysyms(&keysyms).await,
            Input::X11(x11) => tokio::task::spawn_blocking(move || x11.tap_keysyms(&keysyms)).await?,
            Input::Unavailable(why) => bail!("no way to type on this desktop: {why}"),
            Input::Wlr => unreachable!(),
        }
    }

    /// wtype talks to the compositor's virtual-keyboard protocol directly.
    async fn wtype(args: &[&str]) -> Result<()> {
        let out = Command::new("wtype").args(args).output().await.context("running wtype (is it installed?)")?;
        if !out.status.success() {
            bail!("wtype failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod backend {
    use super::Key;
    use crate::sendinput::{self, key_events, text_events};
    use anyhow::Result;

    pub async fn type_text(text: &str) -> Result<()> {
        let events = text_events(text);
        tokio::task::spawn_blocking(move || sendinput::send(&events)).await?
    }

    pub async fn press_key(key: Key, count: u32) -> Result<()> {
        let events = key_events(key, count);
        tokio::task::spawn_blocking(move || sendinput::send(&events)).await?
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod backend {
    use super::Key;
    use anyhow::{bail, Result};

    pub async fn type_text(_: &str) -> Result<()> {
        bail!("typing is not supported on this system yet")
    }

    pub async fn press_key(_: Key, _: u32) -> Result<()> {
        bail!("typing is not supported on this system yet")
    }
}
