//! Word fixes in the Caw for Mac / Android document format:
//! `{"version": 1, "about": "...", "fixes": [{"from", "to", "capitalization", ...}]}`.
//! `from` is a case-insensitive regex (often `a|b` alternatives) with an
//! optional ending (default `'s|s`) kept after the replacement; entries run in
//! order, each over the previous one's output. Same rules as Caw for Android.

use fancy_regex::Regex;
use serde::Deserialize;

pub const DEFAULT_ENDINGS: &str = "'s|s";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Capitalization {
    /// The replacement always starts with a capital.
    Always,
    /// The replacement starts with a capital when the heard word did.
    #[default]
    MatchHeard,
    /// The replacement is used exactly as written.
    Exact,
}

impl Capitalization {
    /// Unknown or missing values mean matchHeard.
    pub fn parse(key: Option<&str>) -> Self {
        match key {
            Some("always") => Self::Always,
            Some("exact") => Self::Exact,
            _ => Self::MatchHeard,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WordFix {
    pub from: String,
    pub to: String,
    pub capitalization: Capitalization,
    pub whole_word: bool,
    pub endings: String,
}

impl WordFix {
    pub fn new(from: &str, to: &str, capitalization: Capitalization) -> Self {
        Self { from: from.into(), to: to.into(), capitalization, whole_word: true, endings: DEFAULT_ENDINGS.into() }
    }
}

#[derive(Deserialize)]
struct RawDocument {
    fixes: Vec<RawFix>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFix {
    from: String,
    to: String,
    capitalization: Option<String>,
    whole_word: Option<bool>,
    endings: Option<String>,
}

/// Reads the fixes from a word-fixes document.
pub fn parse(json: &str) -> serde_json::Result<Vec<WordFix>> {
    let doc: RawDocument = serde_json::from_str(json)?;
    Ok(doc
        .fixes
        .into_iter()
        .map(|f| WordFix {
            from: f.from,
            to: f.to,
            capitalization: Capitalization::parse(f.capitalization.as_deref()),
            whole_word: f.whole_word.unwrap_or(true),
            endings: f.endings.unwrap_or_else(|| DEFAULT_ENDINGS.into()),
        })
        .collect())
}

/// A compiled list of fixes.
#[derive(Default)]
pub struct WordFixes {
    compiled: Vec<(Regex, WordFix)>,
    /// The `from` of every entry whose regex would not compile.
    pub invalid: Vec<String>,
}

/// Named instead of numbered so a capture group inside `from` cannot steal group 1.
const ENDING_GROUP: &str = "cawEnding";

impl WordFixes {
    pub fn new(fixes: Vec<WordFix>) -> Self {
        let mut out = Self::default();
        for fix in fixes {
            if fix.from.trim().is_empty() {
                continue;
            }
            match Regex::new(&pattern_source(&fix)) {
                Ok(re) => out.compiled.push((re, fix)),
                Err(_) => out.invalid.push(fix.from),
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.compiled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (re, fix) in &self.compiled {
            out = replace_all(&out, re, fix);
        }
        out
    }
}

fn pattern_source(fix: &WordFix) -> String {
    // Java's CASE_INSENSITIVE | UNICODE_CASE.
    let core = format!("(?:{})(?P<{ENDING_GROUP}>{})?", fix.from, fix.endings);
    if fix.whole_word {
        format!("(?i)(?<![A-Za-z]){core}(?![A-Za-z])")
    } else {
        format!("(?i){core}")
    }
}

fn replace_all(text: &str, re: &Regex, fix: &WordFix) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in re.captures_iter(text) {
        // A pattern that hits the matcher's backtrack limit leaves the text as it was.
        let Ok(caps) = caps else { return text.to_string() };
        let m = caps.get(0).expect("group 0");
        out.push_str(&text[last..m.start()]);
        out.push_str(&replacement(m.as_str(), caps.name(ENDING_GROUP).map(|e| e.as_str()), fix));
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

/// `heard` is the whole match including the ending; only its first character is tested.
fn replacement(heard: &str, ending: Option<&str>, fix: &WordFix) -> String {
    let word = match fix.capitalization {
        Capitalization::Always => upper_first(&fix.to),
        Capitalization::MatchHeard if heard.chars().next().is_some_and(char::is_uppercase) => upper_first(&fix.to),
        Capitalization::MatchHeard | Capitalization::Exact => fix.to.clone(),
    };
    word + &ending.unwrap_or("").to_lowercase()
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Capitalization::*;

    fn fix(fixes: Vec<WordFix>) -> WordFixes {
        WordFixes::new(fixes)
    }

    // The Android word-fix tests.
    #[test]
    fn always_capitalises() {
        let f = fix(vec![WordFix::new("akme|ackme", "acme", Always)]);
        assert_eq!(f.apply("ask akme now"), "ask Acme now");
        assert_eq!(f.apply("ask akme's help"), "ask Acme's help");
    }

    #[test]
    fn match_heard_follows_the_heard_first_letter() {
        let f = fix(vec![WordFix::new("colour", "color", MatchHeard)]);
        assert_eq!(f.apply("Colour and colour"), "Color and color");
    }

    #[test]
    fn exact_never_changes_case() {
        let f = fix(vec![WordFix::new("teh|tha", "the", Exact)]);
        assert_eq!(f.apply("Teh rocks"), "the rocks");
    }

    #[test]
    fn whole_word_only_matches_whole_words() {
        let f = fix(vec![WordFix::new("teh", "the", MatchHeard)]);
        assert_eq!(f.apply("tehran and teh"), "tehran and the");
    }

    #[test]
    fn whole_word_off_matches_inside_words() {
        let f = fix(vec![WordFix { whole_word: false, ..WordFix::new("hatch", "hutch", MatchHeard) }]);
        assert_eq!(f.apply("rabbithatch"), "rabbithutch");
    }

    #[test]
    fn default_endings_are_kept() {
        let f = fix(vec![WordFix::new("colour", "color", MatchHeard)]);
        assert_eq!(f.apply("colours"), "colors");
        assert_eq!(f.apply("Colour's"), "Color's");
    }

    #[test]
    fn endings_are_lower_cased() {
        let f = fix(vec![WordFix::new("akme", "acme", Always)]);
        assert_eq!(f.apply("AKMES"), "Acmes");
    }

    #[test]
    fn custom_endings() {
        let f = fix(vec![WordFix { endings: "s?(?:'s)?".into(), ..WordFix::new("widget", "widget", Exact) }]);
        assert_eq!(f.apply("Widgets and Widget's"), "widgets and widget's");
        let none = fix(vec![WordFix { endings: String::new(), ..WordFix::new("colour", "color", MatchHeard) }]);
        assert_eq!(none.apply("colours"), "colours");
    }

    #[test]
    fn lookahead_in_from() {
        let f = fix(vec![WordFix::new("jon|john(?='s)", "jon", Always)]);
        assert_eq!(f.apply("john's johns and jon"), "Jon's johns and Jon");
    }

    #[test]
    fn later_entries_see_earlier_output() {
        let f = fix(vec![WordFix::new("colour", "color", MatchHeard), WordFix::new("color's", "Color's", Always)]);
        assert_eq!(f.apply("colour's toy"), "Color's toy");
    }

    #[test]
    fn bad_and_blank_entries_are_skipped() {
        let f = fix(vec![WordFix::new("(", "x", MatchHeard), WordFix::new("  ", "y", MatchHeard), WordFix::new("teh", "the", MatchHeard)]);
        assert_eq!(f.invalid, vec!["(".to_string()]);
        assert_eq!(f.apply("teh"), "the");
    }

    #[test]
    fn parses_the_document_format() {
        let doc = r#"{"version": 1, "about": "x", "fixes": [
            {"from": "akme|ackme", "to": "acme", "capitalization": "always", "note": "n"},
            {"from": "colour", "to": "color"},
            {"from": "hatch", "to": "hutch", "capitalization": "exact", "wholeWord": false, "endings": ""},
            {"from": "x", "to": "y", "capitalization": "sideways"}
        ]}"#;
        let fixes = parse(doc).unwrap();
        assert_eq!(fixes[0], WordFix::new("akme|ackme", "acme", Always));
        assert_eq!(fixes[1], WordFix::new("colour", "color", MatchHeard));
        assert!(!fixes[2].whole_word && fixes[2].endings.is_empty() && fixes[2].capitalization == Exact);
        assert_eq!(fixes[3].capitalization, MatchHeard);
        assert_eq!(WordFixes::new(fixes).apply("Ask akme about the colours"), "Ask Acme about the colors");
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"fixes": []}"#).unwrap().is_empty());
    }

    #[test]
    fn with_the_cleaner_as_step_nine() {
        let f = fix(vec![WordFix::new("akme|ackme", "acme", Always), WordFix::new("colour", "color", MatchHeard)]);
        let segments = crate::speech::text::join_sentences(&["Ask Akme about the colour."]);
        assert_eq!(crate::speech::cleaner::clean(&segments, |t| f.apply(t)), "Ask Acme about the color.");
        let iphone = fix(vec![WordFix::new("iphone", "iPhone", Exact)]);
        assert_eq!(crate::speech::cleaner::clean("iphone is here", |t| iphone.apply(t)), "iPhone is here.");
    }
}
