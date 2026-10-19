//! Types into the focused window. Linux uses whichever method `linux` picked
//! for this desktop; Windows uses SendInput with Unicode key events; macOS uses
//! Quartz key events carrying Unicode text. All send characters rather than key
//! positions, so any keyboard layout works.
//! Key combos (v4) hold modifiers such as Super (Command on a Mac) while tapping a key.

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
    // v4: keys for shortcuts.
    Space,
    PageUp,
    PageDown,
    Insert,
    /// F1–F12.
    F(u8),
    /// A letter, digit or punctuation key, as its unshifted character on a US keyboard.
    Char(char),
}

/// Punctuation keys: protocol name, character, X keysym name.
pub const PUNCTUATION: [(&str, char, &str); 11] = [
    ("minus", '-', "minus"),
    ("equal", '=', "equal"),
    ("comma", ',', "comma"),
    ("period", '.', "period"),
    ("slash", '/', "slash"),
    ("semicolon", ';', "semicolon"),
    ("apostrophe", '\'', "apostrophe"),
    ("bracket_left", '[', "bracketleft"),
    ("bracket_right", ']', "bracketright"),
    ("backslash", '\\', "backslash"),
    ("grave", '`', "grave"),
];

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
            "space" => Self::Space,
            "page_up" => Self::PageUp,
            "page_down" => Self::PageDown,
            "insert" => Self::Insert,
            "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => Self::F(name[1..].parse()?),
            _ => match name.as_bytes() {
                &[c] if c.is_ascii_lowercase() || c.is_ascii_digit() => Self::Char(c as char),
                _ => match PUNCTUATION.iter().find(|(p, ..)| *p == name) {
                    Some(&(_, c, _)) => Self::Char(c),
                    None => bail!("unknown key {name:?}"),
                },
            },
        })
    }
}

/// Modifier keys, in the order they are pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mod {
    /// The Windows key; Command on a Mac keyboard.
    Super,
    Ctrl,
    Alt,
    Shift,
}

impl Mod {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "super" => Self::Super,
            "ctrl" => Self::Ctrl,
            "alt" => Self::Alt,
            "shift" => Self::Shift,
            other => bail!("unknown modifier {other:?}"),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Super => "super",
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Shift => "shift",
        }
    }
}

/// Checks a key and its modifiers (any order, none twice); the modifiers come
/// back in pressing order.
pub fn parse_combo(key: &str, mods: &[String]) -> Result<(Key, Vec<Mod>)> {
    let key = Key::parse(key)?;
    let mut parsed = mods.iter().map(|m| Mod::parse(m)).collect::<Result<Vec<_>>>()?;
    parsed.sort();
    if parsed.windows(2).any(|w| w[0] == w[1]) {
        bail!("a modifier is listed twice");
    }
    Ok((key, parsed))
}

/// CAW_DRY_RUN=1: log instead of typing (for tests).
pub fn dry_run() -> bool {
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

/// Holds `mods` (from `parse_combo`), taps `key` `count` times, then lets go
/// of the modifiers in reverse order, even if a tap failed.
pub async fn press_key(key: Key, mods: &[Mod], count: u32) -> Result<()> {
    let count = count.clamp(1, 200);
    if dry_run() {
        let held = if mods.is_empty() { String::new() } else { format!(" with {mods:?}") };
        eprintln!("caw-desktop: [dry run] key {key:?}{held} x{count}");
        return Ok(());
    }
    backend::press_key(key, mods, count).await
}

#[cfg(target_os = "linux")]
mod backend {
    use super::{Key, Mod, PUNCTUATION};
    use crate::linux::{self, evdev, keysym, vkbd, Input};
    use anyhow::{bail, Context, Result};
    use std::sync::Once;
    use tokio::process::Command;

    pub async fn type_text(text: &str) -> Result<()> {
        match linux::get() {
            Input::Wlr => wtype(&["--", text]).await,
            input => tap(input, keysym::for_text(text)).await,
        }
    }

    pub async fn press_key(key: Key, mods: &[Mod], count: u32) -> Result<()> {
        match linux::get() {
            // Combos go through our own virtual keyboard, which has an evdev
            // keymap so compositor shortcuts see the right keys; wtype is the fallback.
            Input::Wlr if !mods.is_empty() => {
                let held = mods.to_vec();
                let pressed = tokio::task::spawn_blocking(move || vkbd::Keyboard::open().map(|mut kb| kb.press(key, &held, count))).await?;
                match pressed {
                    Ok(result) => result,
                    Err(why) => {
                        static LOGGED: Once = Once::new();
                        LOGGED.call_once(|| eprintln!("caw-desktop: pressing key combos with wtype, so desktop shortcuts may get the wrong key: {why:#}"));
                        wtype_press(key, mods, count).await
                    }
                }
            }
            Input::Wlr => wtype_press(key, mods, count).await,
            // Bare keys are tapped like typed text; combos hold their modifiers.
            Input::Portal(portal) if !mods.is_empty() => {
                let mods: Vec<u32> = mods.iter().map(|&m| evdev::for_mod(m)).collect();
                portal.tap_combo(&mods, evdev::for_key(key), count).await
            }
            Input::X11(x11) if !mods.is_empty() => {
                let mods: Vec<u32> = mods.iter().map(|&m| keysym::for_mod(m)).collect();
                tokio::task::spawn_blocking(move || x11.tap_combo(&mods, keysym::for_key(key), count)).await?
            }
            input => tap(input, vec![keysym::for_key(key); count as usize]).await,
        }
    }

    /// One wtype run holds the modifiers (-M), taps the key (-k) and releases them (-m).
    async fn wtype_press(key: Key, mods: &[Mod], count: u32) -> Result<()> {
        let name = wtype_name(key);
        let mut args: Vec<&str> = mods.iter().flat_map(|&m| ["-M", wtype_mod(m)]).collect();
        args.extend(std::iter::repeat_n(["-k", name.as_str()], count as usize).flatten());
        args.extend(mods.iter().rev().flat_map(|&m| ["-m", wtype_mod(m)]));
        wtype(&args).await
    }

    fn wtype_name(key: Key) -> String {
        match key {
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
            Key::Space => "space",
            Key::PageUp => "Page_Up",
            Key::PageDown => "Page_Down",
            Key::Insert => "Insert",
            Key::F(n) => return format!("F{n}"),
            Key::Char(c) => match PUNCTUATION.iter().find(|p| p.1 == c) {
                Some(p) => p.2,
                None => return c.to_string(),
            },
        }
        .to_string()
    }

    fn wtype_mod(m: Mod) -> &'static str {
        match m {
            Mod::Super => "logo",
            Mod::Ctrl => "ctrl",
            Mod::Alt => "alt",
            Mod::Shift => "shift",
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
    use super::{Key, Mod};
    use crate::sendinput::{self, combo_events, release_events, text_events};
    use anyhow::Result;

    pub async fn type_text(text: &str) -> Result<()> {
        let events = text_events(text);
        tokio::task::spawn_blocking(move || sendinput::send(&events)).await?
    }

    pub async fn press_key(key: Key, mods: &[Mod], count: u32) -> Result<()> {
        let events = combo_events(key, mods, count);
        let release = release_events(mods);
        tokio::task::spawn_blocking(move || {
            let result = sendinput::send(&events);
            if result.is_err() {
                // Don't leave a modifier held down if only part of the input got through.
                let _ = sendinput::send(&release);
            }
            result
        })
        .await?
    }
}

#[cfg(target_os = "macos")]
mod backend {
    use super::{Key, Mod};
    use crate::cgevent::{self, combo_events, release_events, text_events};
    use anyhow::Result;

    pub async fn type_text(text: &str) -> Result<()> {
        let events = text_events(text);
        tokio::task::spawn_blocking(move || cgevent::post(&events)).await?
    }

    pub async fn press_key(key: Key, mods: &[Mod], count: u32) -> Result<()> {
        let events = combo_events(key, mods, count);
        let release = release_events(mods);
        tokio::task::spawn_blocking(move || {
            let result = cgevent::post(&events);
            if result.is_err() {
                // Don't leave a modifier held down if only part of the input got through.
                let _ = cgevent::post(&release);
            }
            result
        })
        .await?
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod backend {
    use super::{Key, Mod};
    use anyhow::{bail, Result};

    pub async fn type_text(_: &str) -> Result<()> {
        bail!("typing is not supported on this system yet")
    }

    pub async fn press_key(_: Key, _: &[Mod], _: u32) -> Result<()> {
        bail!("typing is not supported on this system yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys() {
        assert_eq!(Key::parse("enter").unwrap(), Key::Enter);
        assert_eq!(Key::parse("t").unwrap(), Key::Char('t'));
        assert_eq!(Key::parse("f").unwrap(), Key::Char('f'));
        assert_eq!(Key::parse("7").unwrap(), Key::Char('7'));
        assert_eq!(Key::parse("f5").unwrap(), Key::F(5));
        assert_eq!(Key::parse("f12").unwrap(), Key::F(12));
        assert_eq!(Key::parse("bracket_left").unwrap(), Key::Char('['));
        assert_eq!(Key::parse("page_down").unwrap(), Key::PageDown);
        for bad in ["T", "f0", "f13", "f05", "f+5", "-", "ab", "", "pageup"] {
            assert!(Key::parse(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn combos_come_back_in_pressing_order() {
        let mods = |m: &[&str]| m.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (key, held) = parse_combo("t", &mods(&["shift", "ctrl"])).unwrap();
        assert_eq!((key, held), (Key::Char('t'), vec![Mod::Ctrl, Mod::Shift]));
        assert_eq!(parse_combo("enter", &[]).unwrap(), (Key::Enter, vec![]));
        assert!(parse_combo("enter", &mods(&["super", "super"])).is_err());
        assert!(parse_combo("enter", &mods(&["meta"])).is_err());
        assert!(parse_combo("nope", &mods(&["ctrl"])).is_err());
    }
}
