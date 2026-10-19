//! Joins the recognizer's segments into one text (as Caw for Android does for Moonshine).

/// Each segment was cut at a pause, so it is its own sentence. Segments are
/// trimmed and joined by one space; a segment that ends without . ! ? (or an
/// ellipsis) gets a full stop when another one follows (a trailing comma,
/// colon, semicolon or dash becomes that full stop), and a segment that starts
/// with a lower-case letter after one is capitalised. The text inside a
/// segment, and the last segment's ending, are left exactly as the model wrote them.
pub fn join_sentences<S: AsRef<str>>(segments: &[S]) -> String {
    let parts: Vec<&str> = segments.iter().map(|s| s.as_ref().trim()).filter(|s| !s.is_empty()).collect();
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let mut p = part.to_string();
        if i > 0 {
            out.push(' ');
            if let Some((at, c)) = p.char_indices().find(|(_, c)| c.is_alphanumeric()) {
                if c.is_lowercase() {
                    p = format!("{}{}{}", &p[..at], c.to_uppercase(), &p[at + c.len_utf8()..]);
                }
            }
        }
        if i < parts.len() - 1 {
            p = end_sentence(&p);
        }
        out.push_str(&p);
    }
    out
}

fn end_sentence(s: &str) -> String {
    let t = s.trim_end_matches([',', ';', ':', '-', '–', '—']).trim_end();
    let Some(last) = t.chars().last() else { return s.to_string() };
    // Closing quotes or brackets after the stop count as ended too: 'He said "go."'
    let core = t.trim_end_matches(['"', '\'', '”', '’', ')', ']']);
    if core.chars().last().is_some_and(|c| ".!?…".contains(c)) {
        return t.to_string();
    }
    if last.is_alphanumeric() || "\"'”’)]".contains(last) {
        format!("{t}.")
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_adds_a_stop_and_a_capital_between_segments() {
        assert_eq!(join_sentences(&[" okay trying again", "let's see "]), "okay trying again. Let's see");
    }

    #[test]
    fn trailing_comma_becomes_a_stop() {
        assert_eq!(join_sentences(&["Well, I tried,", "it works."]), "Well, I tried. It works.");
    }

    #[test]
    fn existing_stops_are_kept() {
        assert_eq!(join_sentences(&["Did it work?", "Yes!", "It did."]), "Did it work? Yes! It did.");
        assert_eq!(join_sentences(&["Wait...", "Okay."]), "Wait... Okay.");
        assert_eq!(join_sentences(&["He said \"go.\"", "so"]), "He said \"go.\" So");
    }

    #[test]
    fn empty_segments_are_skipped() {
        assert_eq!(join_sentences(&["", " Okay. ", "  ", "Let's see."]), "Okay. Let's see.");
        assert_eq!(join_sentences::<&str>(&[]), "");
    }
}
