//! Linux evdev keycodes (linux/input-event-codes.h): key positions on a US
//! keyboard. The portal and `vkbd` take these for key combos, which desktops
//! match by key position plus modifiers.

use crate::typer::{Key, Mod};

pub fn for_key(key: Key) -> u32 {
    match key {
        Key::Escape => 1,
        Key::Backspace => 14,
        Key::Tab => 15,
        Key::Enter => 28,
        Key::Space => 57,
        Key::F(n @ 1..=10) => 58 + u32::from(n),
        Key::F(11) => 87,
        Key::F(12) => 88,
        Key::Home => 102,
        Key::Up => 103,
        Key::PageUp => 104,
        Key::Left => 105,
        Key::Right => 106,
        Key::End => 107,
        Key::Down => 108,
        Key::PageDown => 109,
        Key::Insert => 110,
        Key::Delete => 111,
        Key::F(_) => 0,
        Key::Char(c) => match c {
            '1'..='9' => 2 + (c as u32 - '1' as u32),
            '0' => 11,
            '-' => 12,
            '=' => 13,
            '[' => 26,
            ']' => 27,
            ';' => 39,
            '\'' => 40,
            '`' => 41,
            '\\' => 43,
            ',' => 51,
            '.' => 52,
            '/' => 53,
            c => LETTERS.iter().position(|&l| l == c as u8).map_or(0, |i| LETTER_CODES[i]),
        },
    }
}

/// The letters in keycode order, row by row.
const LETTERS: &[u8; 26] = b"qwertyuiopasdfghjklzxcvbnm";
const LETTER_CODES: [u32; 26] = [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 34, 35, 36, 37, 38, 44, 45, 46, 47, 48, 49, 50];

pub fn for_mod(m: Mod) -> u32 {
    match m {
        Mod::Super => 125, // KEY_LEFTMETA
        Mod::Ctrl => 29,   // KEY_LEFTCTRL
        Mod::Alt => 56,    // KEY_LEFTALT
        Mod::Shift => 42,  // KEY_LEFTSHIFT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycodes() {
        assert_eq!(for_key(Key::Enter), 28);
        assert_eq!(for_key(Key::Char('q')), 16);
        assert_eq!(for_key(Key::Char('a')), 30);
        assert_eq!(for_key(Key::Char('m')), 50);
        assert_eq!(for_key(Key::Char('t')), 20);
        assert_eq!(for_key(Key::Char('1')), 2);
        assert_eq!(for_key(Key::Char('9')), 10);
        assert_eq!(for_key(Key::F(1)), 59);
        assert_eq!(for_key(Key::F(10)), 68);
        assert_eq!(for_key(Key::F(12)), 88);
        assert_eq!(for_mod(Mod::Super), 125);
        // Every key the protocol allows has a keycode.
        let names = ["backspace", "delete", "enter", "tab", "escape", "left", "right", "up", "down", "home", "end", "space", "page_up", "page_down", "insert"];
        let letters = ('a'..='z').chain('0'..='9').map(String::from);
        let f_keys = (1..=12).map(|n| format!("f{n}"));
        let punctuation = crate::typer::PUNCTUATION.iter().map(|p| p.0.to_string());
        for name in names.into_iter().map(String::from).chain(letters).chain(f_keys).chain(punctuation) {
            assert_ne!(for_key(Key::parse(&name).unwrap()), 0, "{name}");
        }
    }
}
