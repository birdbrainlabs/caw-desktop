//! Word fixes: phrases the speech model often mishears, corrected before
//! typing. Stored in ~/.config/caw-desktop/word-fixes.json as
//! {"heard phrase": "replacement"} and re-read on every message, so edits
//! apply immediately. Matching is whole-word and case-insensitive.

use anyhow::Result;
use regex::{Regex, RegexBuilder};
use std::collections::BTreeMap;
use std::path::Path;

pub type Fixes = BTreeMap<String, String>;

pub fn defaults() -> Fixes {
    [
        ("call app", "Caw app"),
        ("call desktop", "Caw Desktop"),
        ("hella board", "HeliBoard"),
        ("heli board", "HeliBoard"),
        ("kd e", "KDE"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

pub fn load(path: &Path) -> Fixes {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(path: &Path, fixes: &Fixes) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(fixes)?)?;
    Ok(())
}

/// Writes the default list the first time, so there is a file to edit.
pub fn ensure_file(path: &Path) -> Result<()> {
    if !path.exists() {
        save(path, &defaults())?;
    }
    Ok(())
}

fn pattern(heard: &str) -> Option<Regex> {
    // Whitespace inside a phrase matches any run of spaces.
    let words: Vec<String> = heard.split_whitespace().map(regex::escape).collect();
    if words.is_empty() {
        return None;
    }
    RegexBuilder::new(&format!(r"\b{}\b", words.join(r"\s+"))).case_insensitive(true).build().ok()
}

/// Longest phrases first, so "call desktop" wins over a shorter overlapping fix.
pub fn apply(text: &str, fixes: &Fixes) -> String {
    let mut ordered: Vec<_> = fixes.iter().collect();
    ordered.sort_by_key(|(heard, _)| std::cmp::Reverse(heard.len()));
    let mut out = text.to_string();
    for (heard, fixed) in ordered {
        if let Some(re) = pattern(heard) {
            out = re.replace_all(&out, regex::NoExpand(fixed)).into_owned();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_only() {
        let fixes = defaults();
        assert_eq!(apply("Open the call app now", &fixes), "Open the Caw app now");
        assert_eq!(apply("I'll call you, recall apps", &fixes), "I'll call you, recall apps");
        assert_eq!(apply("CALL  Desktop works", &fixes), "Caw Desktop works");
        assert_eq!(apply("the Hella Board mic", &fixes), "the HeliBoard mic");
    }

    #[test]
    fn replacement_is_literal() {
        let fixes: Fixes = [("cost".to_string(), "$5".to_string())].into();
        assert_eq!(apply("the cost", &fixes), "the $5");
    }
}
