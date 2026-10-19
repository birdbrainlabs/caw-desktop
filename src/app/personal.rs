//! Editing word-fixes.personal.json, the Caw for Mac / Android document
//! (`{"version": 1, "about": ..., "fixes": [{"from", "to", "capitalization", "note", ...}]}`).
//! Everything the editor doesn't show (`endings`, `wholeWord`, unknown keys)
//! is kept as it was. Saved atomically, readable only by this user.

use anyhow::{bail, Context, Result};
use caw_desktop::speech::wordfix;
use serde_json::{json, Map, Value};
use std::path::Path;

pub const FILE: &str = "word-fixes.personal.json";

/// The capitalization keys, as the document writes them, and names for people.
pub const CAPITALIZATION: &[(&str, &str)] = &[("matchHeard", "Like the heard word"), ("always", "Always a capital"), ("exact", "Exactly as typed")];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    pub from: String,
    pub to: String,
    pub capitalization: String,
    pub note: String,
}

pub struct Doc {
    value: Value,
}

impl Doc {
    /// The document, or an empty one if there is no file yet. A file that
    /// can't be read is an error, so it is never overwritten.
    pub fn load(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self { value: json!({"version": 1, "about": "Caw word fixes: what the speech model hears, and what to type instead.", "fixes": []}) })
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let value: Value = serde_json::from_str(&text).with_context(|| format!("{} isn't valid JSON", path.display()))?;
        if !value.get("fixes").is_some_and(Value::is_array) {
            bail!("{} has no \"fixes\" list", path.display());
        }
        Ok(Self { value })
    }

    fn fixes(&self) -> &Vec<Value> {
        self.value["fixes"].as_array().expect("checked in load")
    }

    fn fixes_mut(&mut self) -> &mut Vec<Value> {
        self.value["fixes"].as_array_mut().expect("checked in load")
    }

    pub fn entries(&self) -> Vec<Entry> {
        let text = |f: &Value, key: &str| f.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        self.fixes()
            .iter()
            .map(|f| Entry {
                from: text(f, "from"),
                to: text(f, "to"),
                capitalization: Some(text(f, "capitalization")).filter(|c| !c.is_empty()).unwrap_or_else(|| "matchHeard".into()),
                note: text(f, "note"),
            })
            .collect()
    }

    pub fn add(&mut self, entry: &Entry) -> Result<()> {
        check(entry)?;
        let mut obj = Map::new();
        fill(&mut obj, entry);
        self.fixes_mut().push(Value::Object(obj));
        Ok(())
    }

    /// Changes entry `i`, which must still be `was` (else the file changed meanwhile).
    pub fn update(&mut self, i: usize, was: &Entry, entry: &Entry) -> Result<()> {
        check(entry)?;
        self.expect(i, was)?;
        if let Some(Value::Object(obj)) = self.fixes_mut().get_mut(i) {
            fill(obj, entry);
        }
        Ok(())
    }

    pub fn remove(&mut self, i: usize, was: &Entry) -> Result<()> {
        self.expect(i, was)?;
        self.fixes_mut().remove(i);
        Ok(())
    }

    fn expect(&self, i: usize, was: &Entry) -> Result<()> {
        if self.entries().get(i) != Some(was) {
            bail!("the word fixes file changed meanwhile; the list is shown again as it is now");
        }
        Ok(())
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(&self.value)?;
        bytes.push(b'\n');
        crate::fsutil::write_private(&config_dir.join(FILE), &bytes)
    }
}

fn fill(obj: &mut Map<String, Value>, entry: &Entry) {
    obj.insert("from".into(), entry.from.trim().into());
    obj.insert("to".into(), entry.to.clone().into());
    obj.insert("capitalization".into(), entry.capitalization.clone().into());
    if entry.note.trim().is_empty() {
        obj.remove("note");
    } else {
        obj.insert("note".into(), entry.note.trim().into());
    }
}

/// A fix must have a heard pattern that compiles, and a capitalization the apps know.
pub fn check(entry: &Entry) -> Result<()> {
    if entry.from.trim().is_empty() {
        bail!("say what is heard");
    }
    if !CAPITALIZATION.iter().any(|(k, _)| *k == entry.capitalization) {
        bail!("unknown capitalization {:?}", entry.capitalization);
    }
    let compiled = wordfix::WordFixes::new(vec![wordfix::WordFix::new(entry.from.trim(), &entry.to, wordfix::Capitalization::parse(Some(&entry.capitalization)))]);
    if !compiled.invalid.is_empty() {
        bail!("\"{}\" isn't a pattern Caw can read (check brackets and | signs)", entry.from.trim());
    }
    Ok(())
}

/// "Heard" text for a word or phrase selected in the last dictation: the
/// words without surrounding punctuation, with pattern characters escaped.
pub fn heard_from_selection(selected: &str) -> String {
    let words = selected.trim_matches(|c: char| !c.is_alphanumeric()).split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for c in words.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("caw-personal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn edits_keep_what_the_editor_does_not_show() {
        let d = dir("keep");
        std::fs::write(
            d.join(FILE),
            r#"{"version": 1, "about": "a", "extra": 5, "fixes": [{"from": "akme", "to": "acme", "capitalization": "always", "endings": "", "note": "n", "x": true}]}"#,
        )
        .unwrap();
        let mut doc = Doc::load(&d).unwrap();
        let was = doc.entries()[0].clone();
        assert_eq!(was, Entry { from: "akme".into(), to: "acme".into(), capitalization: "always".into(), note: "n".into() });
        doc.update(0, &was, &Entry { from: "akme|ackme".into(), to: "Acme".into(), capitalization: "exact".into(), note: String::new() }).unwrap();
        doc.add(&Entry { from: "colour".into(), to: "color".into(), capitalization: "matchHeard".into(), note: "uk".into() }).unwrap();
        doc.save(&d).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join(FILE)).unwrap()).unwrap();
        assert_eq!(v["extra"], 5);
        assert_eq!(v["fixes"][0], json!({"from": "akme|ackme", "to": "Acme", "capitalization": "exact", "endings": "", "x": true}));
        assert_eq!(v["fixes"][1]["note"], "uk");
        // The dictation side reads it.
        let parsed = wordfix::parse(&std::fs::read_to_string(d.join(FILE)).unwrap()).unwrap();
        assert_eq!(wordfix::WordFixes::new(parsed).apply("ask ackme about the colour"), "ask Acme about the color");
        let mut doc = Doc::load(&d).unwrap();
        assert!(doc.remove(0, &was).is_err(), "a stale entry is not removed");
        let now = doc.entries()[0].clone();
        doc.remove(0, &now).unwrap();
        assert_eq!(doc.entries().len(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bad_files_and_fixes_are_refused() {
        let d = dir("bad");
        assert!(Doc::load(&d).unwrap().entries().is_empty());
        std::fs::write(d.join(FILE), "not json").unwrap();
        assert!(Doc::load(&d).is_err());
        let mut doc = Doc { value: json!({"fixes": []}) };
        assert!(doc.add(&Entry { from: "(".into(), to: "x".into(), capitalization: "exact".into(), note: String::new() }).is_err());
        assert!(doc.add(&Entry { from: " ".into(), to: "x".into(), capitalization: "exact".into(), note: String::new() }).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn selection_becomes_a_pattern() {
        assert_eq!(heard_from_selection(" Akme, "), "akme");
        assert_eq!(heard_from_selection("node.js  rocks."), "node\\.js rocks");
        assert_eq!(heard_from_selection("“C++?”"), "c");
    }
}
