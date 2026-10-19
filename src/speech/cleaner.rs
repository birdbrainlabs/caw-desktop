//! Tidies a dictated transcript: stutter fragments, filler words and repeated
//! words go; spacing, capitals and the closing full stop are fixed. A port of
//! the Caw for Android cleaner (itself a port of Caw for Mac's), steps in the
//! same order. Every step only drops or re-cases words, never invents one.

use fancy_regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

type Captures<'t> = fancy_regex::Captures<'t, str>;

/// Cleans `raw`. `word_fixes` is step 9 (the user's word fixes): it runs after
/// capitalisation and before the closing full stop.
pub fn clean(raw: &str, word_fixes: impl Fn(&str) -> String) -> String {
    let mut text = spell_out_ordinals(raw);
    text = remove_stutters(&text);
    text = remove_fillers(&text);
    text = collapse_repeats(&text);
    text = normalize_spacing(&text);
    text = replace(&CLAUSE_COMMA, &text, |_| " ".into());
    text = break_run_ons(&text);
    text = capitalize_sentences(&text);
    text = word_fixes(&text);
    add_terminal_punctuation(&text)
}

/// Every pattern runs case-insensitively, as on Android (Java's CASE_INSENSITIVE | UNICODE_CASE,
/// which also makes back-references match regardless of case).
fn rx(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}")).expect("cleaner pattern")
}

/// Replaces every match. A pattern that hits the matcher's backtrack limit leaves the text as it was.
fn replace(re: &Regex, text: &str, f: impl Fn(&Captures) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in re.captures_iter(text) {
        let Ok(caps) = caps else { return text.to_string() };
        let m = caps.get(0).expect("group 0");
        out.push_str(&text[last..m.start()]);
        out.push_str(&f(&caps));
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

fn group<'t>(caps: &Captures<'t>, i: usize) -> &'t str {
    caps.get(i).map_or("", |m| m.as_str())
}

/// Lower-case with diacritics removed, for comparing stutter fragments.
fn fold(s: &str) -> String {
    s.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect::<String>().to_lowercase()
}

fn core(word: &str) -> String {
    word.to_lowercase().chars().filter(|c| c.is_alphabetic()).collect()
}

// 1
fn spell_out_ordinals(input: &str) -> String {
    if !input.chars().any(|c| c.is_ascii_digit()) {
        return input.to_string();
    }
    let mut text = input.to_string();
    for (re, word) in ORDINALS.iter() {
        text = replace(re, &text, |_| word.to_string());
    }
    text
}

// 2a, 2b, 2c
fn remove_stutters(input: &str) -> String {
    let mut text = replace(&INLINE_STUTTER, input, |caps| {
        let whole = group(caps, 0);
        let pieces: Vec<&str> = whole.split('-').collect();
        let last = fold(pieces[pieces.len() - 1]);
        if pieces.len() > 1 && pieces[..pieces.len() - 1].iter().all(|p| last.starts_with(&fold(p))) {
            pieces[pieces.len() - 1].to_string()
        } else {
            whole.to_string()
        }
    });
    for _ in 0..8 {
        let next = replace(&SEPARATED_STUTTER, &text, drop_if_prefix);
        if next == text {
            break;
        }
        text = next;
    }
    replace(&TRAILING_OFF, &text, drop_if_prefix)
}

fn drop_if_prefix(caps: &Captures) -> String {
    if fold(group(caps, 2)).starts_with(&fold(group(caps, 1))) {
        String::new()
    } else {
        group(caps, 0).to_string()
    }
}

// 3
fn remove_fillers(input: &str) -> String {
    let text = replace(&PARENTHETICAL_FILLER, input, |_| ",".into());
    replace(&FILLER, &text, |_| String::new())
}

// 4a, 4b, 4c
fn collapse_repeats(input: &str) -> String {
    let mut text = input.to_string();
    for _ in 0..4 {
        let next = replace(&REPEATED_PHRASE, &text, |caps| {
            let whole = group(caps, 0);
            let phrase = group(caps, 1);
            // "New York, New York" survives.
            if whole.contains(',') && phrase.chars().next().is_some_and(char::is_uppercase) {
                whole.to_string()
            } else {
                phrase.to_string()
            }
        });
        if next == text {
            break;
        }
        text = next;
    }
    text = replace(&PUNCTUATED_REPEAT, &text, |caps| group(caps, 1).to_string());
    replace(&REPEATED_WORD, &text, |caps| group(caps, 1).to_string())
}

// 5
fn normalize_spacing(input: &str) -> String {
    let mut text = replace(&SPACE_BEFORE_PUNCTUATION, input, |caps| group(caps, 1).to_string());
    text = replace(&REPEATED_COMMA, &text, |_| ", ".into());
    text = replace(&COMMA_AFTER_SENTENCE_END, &text, |caps| format!("{} ", group(caps, 1)));
    text = replace(&LEADING_COMMA, &text, |_| String::new());
    text = replace(&LEADING_PUNCTUATION, &text, |_| String::new());
    text = replace(&REPEATED_SPACE, &text, |_| " ".into());
    // As on the Mac: the newline goes too.
    text = replace(&SPACE_BEFORE_NEWLINE, &text, |_| String::new());
    text.trim().to_string()
}

// 7
fn break_run_ons(text: &str) -> String {
    let mut words: Vec<String> = text.split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
    if words.len() <= 10 {
        return text.to_string();
    }
    let mut distance = 0;
    for index in 0..words.len() {
        let previous = if index > 0 { words[index - 1].clone() } else { String::new() };
        if previous.ends_with(['.', '?', '!']) {
            distance = 0;
        }
        let word = core(&words[index]);
        let remaining = words.len() - index;
        let previous_ends_plain = previous.chars().last().is_some_and(char::is_alphanumeric);
        let break_here = distance >= 9
            && remaining >= 3
            && STARTERS.contains(&word.as_str())
            && previous_ends_plain
            && !BLOCKED_BEFORE.contains(&core(&previous).as_str())
            && ((word != "and" && word != "so") || (index + 1 < words.len() && CONTINUERS.contains(&core(&words[index + 1]).as_str())));
        if break_here {
            words[index - 1] = format!("{previous}.");
            distance = 0;
        } else {
            distance += 1;
        }
    }
    words.join(" ")
}

// 8
fn capitalize_sentences(text: &str) -> String {
    replace(&SENTENCE_START, text, |caps| {
        let m = group(caps, 0);
        let mut chars: Vec<char> = m.chars().collect();
        let last = chars.pop().unwrap_or_default();
        chars.into_iter().chain(last.to_uppercase()).collect()
    })
}

// 10
fn add_terminal_punctuation(text: &str) -> String {
    if text.chars().last().is_some_and(char::is_alphanumeric) {
        format!("{text}.")
    } else {
        text.to_string()
    }
}

static ORDINALS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        ("1st", "first"),
        ("2nd", "second"),
        ("3rd", "third"),
        ("4th", "fourth"),
        ("5th", "fifth"),
        ("6th", "sixth"),
        ("7th", "seventh"),
        ("8th", "eighth"),
        ("9th", "ninth"),
        ("10th", "tenth"),
    ]
    .into_iter()
    .map(|(digits, word)| (rx(&format!("(?<![0-9A-Za-z]){digits}(?![0-9A-Za-z])")), word))
    .collect()
});

static INLINE_STUTTER: LazyLock<Regex> = LazyLock::new(|| rx(r"\b(?:\p{L}{1,3}-){1,}\p{L}+\b"));
static SEPARATED_STUTTER: LazyLock<Regex> = LazyLock::new(|| rx(r"\b(\p{L}{1,3})-[\t ]+(?=(\p{L}+)\b)"));
static TRAILING_OFF: LazyLock<Regex> = LazyLock::new(|| rx(r"\b(\p{L}[\p{L}\p{N}'’]*)[\t ]*(?:\.{2,}|…)[\t ]+(?=(\p{L}[\p{L}\p{N}'’]*))"));

static PARENTHETICAL_FILLER: LazyLock<Regex> = LazyLock::new(|| rx(r",\s*u(?:h+m*|m+)\s*,"));
static FILLER: LazyLock<Regex> = LazyLock::new(|| rx(r"(?<![\p{L}\p{N}'’\-])u(?:h+m*|m+)(?![\p{L}\p{N}'’\-])"));

static REPEATED_PHRASE: LazyLock<Regex> =
    LazyLock::new(|| rx(r"\b((?:\p{L}[\p{L}\p{N}'’]*)(?:[\t ]+\p{L}[\p{L}\p{N}'’]*){1,11})[\t ]*,?[\t ]+\1(?![\p{L}\p{N}'’])"));
static PUNCTUATED_REPEAT: LazyLock<Regex> =
    LazyLock::new(|| rx(r"\b((?:\p{L}[\p{L}\p{N}'’]*)(?:[\t ]+\p{L}[\p{L}\p{N}'’]*){0,11})\b(?:[\t ]*[.,][\t ]+\1\b){2,}"));
static REPEATED_WORD: LazyLock<Regex> = LazyLock::new(|| rx(r"\b(\p{L}[\p{L}\p{N}]*(?:['’][\p{L}\p{N}]+)?)\b(?:[\t ]*,?[\t ]+\1\b)+"));

static SPACE_BEFORE_PUNCTUATION: LazyLock<Regex> = LazyLock::new(|| rx(r"[\t ]+([,.;:!?])"));
static REPEATED_COMMA: LazyLock<Regex> = LazyLock::new(|| rx(r",(?:[\t ]*,)+[\t ]*"));
static COMMA_AFTER_SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| rx(r"([.!?])[\t ]*,[\t ]*"));
static LEADING_COMMA: LazyLock<Regex> = LazyLock::new(|| rx(r"(?m)^[\t ]*,[\t ]*"));
static LEADING_PUNCTUATION: LazyLock<Regex> = LazyLock::new(|| rx(r"\A[\t ]*[.,;:!?]+[\t ]*"));
static REPEATED_SPACE: LazyLock<Regex> = LazyLock::new(|| rx(r"[\t ]{2,}"));
static SPACE_BEFORE_NEWLINE: LazyLock<Regex> = LazyLock::new(|| rx(r"[\t ]+\n"));

static CLAUSE_COMMA: LazyLock<Regex> = LazyLock::new(|| rx(r",\s+(?=(?:and|but)\b)"));
static SENTENCE_START: LazyLock<Regex> = LazyLock::new(|| rx(r"(?:\A|(?<=[.!?])[\t ]+|(?<=\n))\p{Ll}"));

const STARTERS: &[&str] = &["i", "and", "so", "we", "lets", "if", "why", "what", "please"];
const CONTINUERS: &[&str] = &["i", "we", "it", "the", "that", "this", "then", "you", "my", "a", "an", "im", "lets", "were", "maybe", "now", "theres"];
const BLOCKED_BEFORE: &[&str] = &[
    "and", "but", "or", "so", "that", "which", "if", "when", "because", "as", "than", "then", "like", "to", "of", "in", "on", "at", "for", "with", "from", "by", "about",
    "into", "over", "after", "before", "while", "where", "a", "an", "the", "my", "your", "his", "her", "its", "our", "their", "me", "do", "does", "did", "can", "could",
    "will", "would", "should", "is", "are", "was", "am", "be", "been", "not", "dont", "didnt", "cant", "wont", "let", "make", "makes", "help", "tell", "told", "ask",
    "asked", "say", "said", "think", "know", "see", "want", "need", "mean", "guess", "hope", "wish", "sure", "how", "who", "why", "what", "maybe",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::text::join_sentences;

    fn c(raw: &str) -> String {
        clean(raw, |s| s.to_string())
    }

    #[test]
    fn every_pattern_compiles() {
        for re in [
            &*INLINE_STUTTER, &*SEPARATED_STUTTER, &*TRAILING_OFF, &*PARENTHETICAL_FILLER, &*FILLER, &*REPEATED_PHRASE, &*PUNCTUATED_REPEAT, &*REPEATED_WORD,
            &*SPACE_BEFORE_PUNCTUATION, &*REPEATED_COMMA, &*COMMA_AFTER_SENTENCE_END, &*LEADING_COMMA, &*LEADING_PUNCTUATION, &*REPEATED_SPACE,
            &*SPACE_BEFORE_NEWLINE, &*CLAUSE_COMMA, &*SENTENCE_START,
        ] {
            assert!(re.is_match("x").is_ok());
        }
        assert_eq!(ORDINALS.len(), 10);
    }

    // The Android cleaner's tests.
    #[test]
    fn fillers_and_inline_stutters() {
        assert_eq!(c("um I w-want to go"), "I want to go.");
    }

    #[test]
    fn hyphenated_words_survive() {
        assert_eq!(c("a well-known trick"), "A well-known trick.");
    }

    #[test]
    fn separated_stutter() {
        assert_eq!(c("I w- want it"), "I want it.");
        assert_eq!(c("I w- w- want it"), "I want it.");
    }

    #[test]
    fn trailing_off() {
        assert_eq!(c("there... there's a cat"), "There's a cat.");
    }

    #[test]
    fn parenthetical_filler() {
        assert_eq!(c("so, um, we left"), "So, we left.");
    }

    #[test]
    fn repeated_words_collapse() {
        assert_eq!(c("the the cat sat"), "The cat sat.");
        // Back-references ignore case, as in Java.
        assert_eq!(c("The the cat sat"), "The cat sat.");
    }

    #[test]
    fn repeated_phrase_collapses_but_new_york_survives() {
        assert_eq!(c("I went to I went to the shop"), "I went to the shop.");
        assert_eq!(c("New York, New York"), "New York, New York.");
    }

    #[test]
    fn small_ordinals_are_spelled_out() {
        assert_eq!(c("the 2nd and 11th place"), "The second and 11th place.");
    }

    #[test]
    fn comma_before_and_is_removed() {
        assert_eq!(c("I like apples, and pears"), "I like apples and pears.");
    }

    #[test]
    fn long_run_on_is_broken() {
        assert_eq!(
            c("I went to the store and bought some milk then we went home and we slept"),
            "I went to the store and bought some milk then we went home. And we slept."
        );
    }

    #[test]
    fn word_fixes_run_after_capitals_and_before_the_full_stop() {
        assert_eq!(clean("iphone is here", |s| s.replace("Iphone", "iPhone")), "iPhone is here.");
    }

    #[test]
    fn existing_punctuation_is_kept() {
        assert_eq!(c("is it done?"), "Is it done?");
    }

    // Android's Moonshine text tests: joined segments, then the cleaner.
    fn run(segments: &[&str]) -> String {
        c(&join_sentences(segments))
    }

    #[test]
    fn moonshine_segments() {
        assert_eq!(run(&["Okay, trying again.", "Let's see."]), "Okay, trying again. Let's see.");
        assert_eq!(run(&["Well, I tried,", "it works."]), "Well, I tried. It works.");
        assert_eq!(run(&["Did it work?", "Yes!", "It did."]), "Did it work? Yes! It did.");
        assert_eq!(run(&["This is working."]), "This is working.");
        assert_eq!(run(&["One.", "Two."]), "One. Two.");
        assert_eq!(run(&["I told Sam about the iPad in New York."]), "I told Sam about the iPad in New York.");
        assert_eq!(run(&["Okay.", "Let's see"]), "Okay. Let's see.");
        assert_eq!(run(&["", " Okay. ", "  ", "Let's see."]), "Okay. Let's see.");
    }

    #[test]
    fn empty_stays_empty() {
        assert_eq!(c(""), "");
        assert_eq!(c("  um  "), "");
    }
}
