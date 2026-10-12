//! X keysyms: the layout-independent key names that both the remote-desktop
//! portal and X11 understand.

use crate::typer::{Key, Mod};

pub const SHIFT_L: u32 = 0xffe1;

pub fn for_key(key: Key) -> u32 {
    match key {
        Key::Backspace => 0xff08,
        Key::Tab => 0xff09,
        Key::Enter => 0xff0d,
        Key::Escape => 0xff1b,
        Key::Home => 0xff50,
        Key::Left => 0xff51,
        Key::Up => 0xff52,
        Key::Right => 0xff53,
        Key::Down => 0xff54,
        Key::End => 0xff57,
        Key::Delete => 0xffff,
        Key::Space => 0x20,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Insert => 0xff63,
        Key::F(n) => 0xffbe + u32::from(n) - 1,
        // Letters, digits and ASCII punctuation are their own keysyms.
        Key::Char(c) => c as u32,
    }
}

pub fn for_mod(m: Mod) -> u32 {
    match m {
        Mod::Super => 0xffeb,
        Mod::Ctrl => 0xffe3,
        Mod::Alt => 0xffe9,
        Mod::Shift => SHIFT_L,
    }
}

/// The keysyms to tap, one per character. Newlines and tabs become Enter and
/// Tab; ASCII and Latin-1 have their own keysyms; everything else uses the
/// Unicode range (0x01000000 + code point).
pub fn for_text(text: &str) -> Vec<u32> {
    text.chars()
        .filter(|&c| c != '\r')
        .map(|c| match c {
            '\n' => for_key(Key::Enter),
            '\t' => for_key(Key::Tab),
            c if matches!(c as u32, 0x20..=0x7e | 0xa0..=0xff) => c as u32,
            c => 0x0100_0000 + c as u32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_to_keysyms() {
        assert_eq!(for_text("Aé\r\n€"), vec![0x41, 0xe9, 0xff0d, 0x0100_20ac]);
        assert_eq!(for_key(Key::Backspace), 0xff08);
        assert_eq!(for_key(Key::F(1)), 0xffbe);
        assert_eq!(for_key(Key::F(12)), 0xffc9);
        assert_eq!(for_key(Key::Char('t')), 0x74);
        assert_eq!(for_key(Key::Char('[')), 0x5b);
        assert_eq!(for_mod(Mod::Super), 0xffeb);
    }
}
