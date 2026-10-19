//! Shortcuts: named key combos the phone shows as buttons (protocol v4), e.g.
//! Terminal = Super+Enter. Stored in ~/.config/caw-desktop/shortcuts.json and
//! re-read whenever the phone asks, so `caw-desktop shortcuts` edits reach the
//! phone the next time it connects. None by default: Super+Enter opens a
//! terminal on some Linux desktops but starts Narrator on Windows.

use crate::typer;
use anyhow::{anyhow, bail, Result};
use caw_desktop::proto::Shortcut;
use std::path::Path;

pub const MAX_SHORTCUTS: usize = 48;
pub const MAX_NAME: usize = 30;

pub fn load(path: &Path) -> Vec<Shortcut> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(path: &Path, list: &[Shortcut]) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(list)?)?;
    Ok(())
}

/// Checks the whole list and returns it tidied: names trimmed, modifiers in
/// pressing order. Duplicate names are fine.
pub fn check(list: Vec<Shortcut>) -> Result<Vec<Shortcut>> {
    if list.len() > MAX_SHORTCUTS {
        bail!("at most {MAX_SHORTCUTS} shortcuts");
    }
    list.into_iter()
        .map(|s| {
            let name = s.name.trim().to_string();
            if !(1..=MAX_NAME).contains(&name.chars().count()) {
                bail!("each shortcut needs a name of 1–{MAX_NAME} characters");
            }
            let (_, mods) = typer::parse_combo(&s.key, &s.mods).map_err(|e| anyhow!("shortcut {name:?}: {e}"))?;
            Ok(Shortcut { name, key: s.key, mods: mods.iter().map(|m| m.name().to_string()).collect() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(name: &str, key: &str, mods: &[&str]) -> Shortcut {
        Shortcut { name: name.into(), key: key.into(), mods: mods.iter().map(|m| m.to_string()).collect() }
    }

    #[test]
    fn tidies_a_valid_list() {
        let list = check(vec![shortcut("  Terminal ", "enter", &["super"]), shortcut("Tab", "t", &["shift", "ctrl"]), shortcut("Tab", "f5", &[])]).unwrap();
        assert_eq!(list, vec![shortcut("Terminal", "enter", &["super"]), shortcut("Tab", "t", &["ctrl", "shift"]), shortcut("Tab", "f5", &[])]);
    }

    #[test]
    fn refuses_bad_entries() {
        assert!(check(vec![shortcut("   ", "enter", &[])]).is_err());
        assert!(check(vec![shortcut(&"x".repeat(31), "enter", &[])]).is_err());
        assert!(check(vec![shortcut(&"é".repeat(30), "enter", &[])]).is_ok());
        assert!(check(vec![shortcut("A", "return", &[])]).is_err());
        assert!(check(vec![shortcut("A", "enter", &["meta"])]).is_err());
        assert!(check(vec![shortcut("A", "enter", &["ctrl", "ctrl"])]).is_err());
        assert!(check(vec![shortcut("A", "a", &[]); MAX_SHORTCUTS]).is_ok());
        assert!(check(vec![shortcut("A", "a", &[]); MAX_SHORTCUTS + 1]).is_err());
    }
}
