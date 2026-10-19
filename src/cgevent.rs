//! macOS keyboard and mouse input through Quartz events (CGEvent). The event
//! lists are built by plain functions, tested on every platform; only `post`
//! touches macOS.
//!
//! Text is typed as Unicode strings attached to key events, so any keyboard
//! layout works. Keys for shortcuts are pressed by position on a US keyboard
//! (kVK codes), with Super as the Command key.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::pointer::Button;
use crate::typer::{Key, Mod};

// Modifier flags (CGEventFlags).
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_OPTION: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;
/// Real keyboards set this on the arrow keys.
pub const FLAG_NUMPAD: u64 = 0x0020_0000;
/// Real keyboards set this on the keys an Apple keyboard reaches with Fn:
/// arrows, Home, End, Page Up/Down, Forward Delete, Help and F1–F12.
pub const FLAG_FN: u64 = 0x0080_0000;

// Virtual keycodes (HIToolbox Events.h). The ANSI ones are positions on a US keyboard.
const KVK_RETURN: u16 = 0x24;
const KVK_TAB: u16 = 0x30;
const KVK_SPACE: u16 = 0x31;
const KVK_DELETE: u16 = 0x33; // Backspace
const KVK_ESCAPE: u16 = 0x35;
const KVK_COMMAND: u16 = 0x37;
const KVK_SHIFT: u16 = 0x38;
const KVK_OPTION: u16 = 0x3A;
const KVK_CONTROL: u16 = 0x3B;
const KVK_HELP: u16 = 0x72; // where Insert is on a PC keyboard
const KVK_HOME: u16 = 0x73;
const KVK_PAGE_UP: u16 = 0x74;
const KVK_FORWARD_DELETE: u16 = 0x75;
const KVK_END: u16 = 0x77;
const KVK_PAGE_DOWN: u16 = 0x79;
const KVK_LEFT: u16 = 0x7B;
const KVK_RIGHT: u16 = 0x7C;
const KVK_DOWN: u16 = 0x7D;
const KVK_UP: u16 = 0x7E;
/// F1 to F12.
const KVK_F: [u16; 12] = [0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F];
/// a to z.
const KVK_LETTERS: [u16; 26] = [
    0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
];
/// 0 to 9.
const KVK_DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];

/// How many UTF-16 units one key event carries; macOS takes at most 20.
pub const TEXT_CHUNK: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    /// A key by position, with the modifier flags held while it is pressed.
    Key { code: u16, down: bool, flags: u64 },
    /// A modifier key going down or up; `flags` are the modifiers held afterwards.
    Modifier { code: u16, flags: u64 },
    /// Text typed as-is whatever the keyboard layout: a key down and up carrying it.
    Text(Vec<u16>),
    /// The pointer to (x, y) in global display points, moved by (dx, dy).
    /// While a button is held, this is a drag with that button.
    Move { x: f64, y: f64, dx: f64, dy: f64, held: Option<Button> },
    /// `clicks` is 1 for a single click, 2 for the second click of a double click, …
    Button { button: Button, down: bool, x: f64, y: f64, clicks: i64 },
    /// In pixels; positive scrolls up (vertical) or left (horizontal), as macOS counts.
    Scroll { vertical: i32, horizontal: i32 },
}

/// The keycode for `key`, and the flags a real keyboard adds to it.
pub fn keycode(key: Key) -> (u16, u64) {
    match key {
        Key::Backspace => (KVK_DELETE, 0),
        Key::Delete => (KVK_FORWARD_DELETE, FLAG_FN),
        Key::Enter => (KVK_RETURN, 0),
        Key::Tab => (KVK_TAB, 0),
        Key::Escape => (KVK_ESCAPE, 0),
        Key::Left => (KVK_LEFT, FLAG_NUMPAD | FLAG_FN),
        Key::Right => (KVK_RIGHT, FLAG_NUMPAD | FLAG_FN),
        Key::Up => (KVK_UP, FLAG_NUMPAD | FLAG_FN),
        Key::Down => (KVK_DOWN, FLAG_NUMPAD | FLAG_FN),
        Key::Home => (KVK_HOME, FLAG_FN),
        Key::End => (KVK_END, FLAG_FN),
        Key::Space => (KVK_SPACE, 0),
        Key::PageUp => (KVK_PAGE_UP, FLAG_FN),
        Key::PageDown => (KVK_PAGE_DOWN, FLAG_FN),
        Key::Insert => (KVK_HELP, FLAG_FN),
        Key::F(n) => (KVK_F[usize::from(n.clamp(1, 12)) - 1], FLAG_FN),
        Key::Char(c) => (
            match c {
                'a'..='z' => KVK_LETTERS[(c as u8 - b'a') as usize],
                '0'..='9' => KVK_DIGITS[(c as u8 - b'0') as usize],
                '-' => 0x1B,
                '=' => 0x18,
                ',' => 0x2B,
                '.' => 0x2F,
                '/' => 0x2C,
                ';' => 0x29,
                '\'' => 0x27,
                '[' => 0x21,
                ']' => 0x1E,
                '\\' => 0x2A,
                '`' => 0x32,
                // Key::parse only makes the characters above.
                _ => KVK_SPACE,
            },
            0,
        ),
    }
}

/// The modifier's keycode and flag. Super is the Command key.
pub fn modifier(m: Mod) -> (u16, u64) {
    match m {
        Mod::Super => (KVK_COMMAND, FLAG_COMMAND),
        Mod::Ctrl => (KVK_CONTROL, FLAG_CONTROL),
        Mod::Alt => (KVK_OPTION, FLAG_OPTION),
        Mod::Shift => (KVK_SHIFT, FLAG_SHIFT),
    }
}

fn tap(code: u16, flags: u64) -> [Ev; 2] {
    [Ev::Key { code, down: true, flags }, Ev::Key { code, down: false, flags }]
}

/// Holds `mods` (in pressing order), taps the key `count` times with them
/// held, then releases the modifiers in reverse order.
pub fn combo_events(key: Key, mods: &[Mod], count: u32) -> Vec<Ev> {
    let mut events = Vec::new();
    let mut held = 0;
    for &m in mods {
        let (code, flag) = modifier(m);
        held |= flag;
        events.push(Ev::Modifier { code, flags: held });
    }
    let (code, extra) = keycode(key);
    for _ in 0..count {
        events.extend(tap(code, held | extra));
    }
    events.extend(release_events(mods));
    events
}

/// Lets go of `mods` in reverse order.
pub fn release_events(mods: &[Mod]) -> Vec<Ev> {
    let mut held: u64 = mods.iter().map(|&m| modifier(m).1).fold(0, |a, b| a | b);
    mods.iter()
        .rev()
        .map(|&m| {
            let (code, flag) = modifier(m);
            held &= !flag;
            Ev::Modifier { code, flags: held }
        })
        .collect()
}

/// Newlines and tabs become real Return and Tab presses (apps ignore them as
/// text); everything else is typed as Unicode, up to 20 UTF-16 units per event,
/// never splitting a character.
pub fn text_events(text: &str) -> Vec<Ev> {
    let mut events = Vec::new();
    let mut chunk: Vec<u16> = Vec::with_capacity(TEXT_CHUNK);
    let flush = |chunk: &mut Vec<u16>, events: &mut Vec<Ev>| {
        if !chunk.is_empty() {
            events.push(Ev::Text(std::mem::take(chunk)));
        }
    };
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' | '\t' => {
                flush(&mut chunk, &mut events);
                events.extend(tap(if c == '\n' { KVK_RETURN } else { KVK_TAB }, 0));
            }
            c => {
                let mut buf = [0; 2];
                let units = c.encode_utf16(&mut buf);
                if chunk.len() + units.len() > TEXT_CHUNK {
                    flush(&mut chunk, &mut events);
                }
                chunk.extend_from_slice(units);
            }
        }
    }
    flush(&mut chunk, &mut events);
    events
}

/// A display's area in global points (origin top left of the main display).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn clamp(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (x.clamp(self.x, self.x + self.w - 1.0), y.clamp(self.y, self.y + self.h - 1.0))
    }
}

/// Where the pointer at `from` ends up after moving by (dx, dy): anywhere on a
/// display, or else stopped at the edge of the display it was on.
pub fn move_within(from: (f64, f64), dx: f64, dy: f64, displays: &[Area]) -> (f64, f64) {
    let to = (from.0 + dx, from.1 + dy);
    if displays.is_empty() || displays.iter().any(|d| d.contains(to)) {
        return to;
    }
    let home = displays.iter().find(|d| d.contains(from)).unwrap_or(&displays[0]);
    home.clamp(to)
}

/// Counts clicks the way a real mouse does, so a quick second tap is a double click.
#[derive(Debug, Default)]
pub struct Clicks {
    last: Option<(Button, std::time::Instant, (f64, f64))>,
    count: i64,
}

impl Clicks {
    /// The click count for a press of `button` at `at`, `now`.
    pub fn press(&mut self, button: Button, at: (f64, f64), now: std::time::Instant, interval: std::time::Duration) -> i64 {
        let again = self.last.is_some_and(|(b, t, (x, y))| b == button && now.duration_since(t) <= interval && (x - at.0).abs() <= 4.0 && (y - at.1).abs() <= 4.0);
        self.count = if again { self.count + 1 } else { 1 };
        self.last = Some((button, now, at));
        self.count
    }

    /// The count to put on the release that ends the current click.
    pub fn current(&self) -> i64 {
        self.count.max(1)
    }
}

/// Whether this process may post keyboard and mouse events: the user has
/// allowed it in System Settings → Privacy & Security → Accessibility.
/// Asked of macOS at most once a second, since every mouse move checks it.
#[cfg(target_os = "macos")]
pub fn allowed() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, yes)) = *last {
        if at.elapsed() < Duration::from_secs(1) {
            return yes;
        }
    }
    let yes = unsafe { objc2_application_services::AXIsProcessTrusted() };
    *last = Some((Instant::now(), yes));
    yes
}

/// Checks `allowed`, and if not, has macOS show its dialog pointing the user
/// to the Accessibility settings (macOS shows it once per app).
#[cfg(target_os = "macos")]
pub fn ask_for_access() -> bool {
    use objc2_application_services::{kAXTrustedCheckOptionPrompt, AXIsProcessTrustedWithOptions};
    use objc2_core_foundation::{kCFBooleanTrue, CFDictionary};
    let Some(yes) = (unsafe { kCFBooleanTrue }) else { return allowed() };
    let options = CFDictionary::from_slices(&[unsafe { kAXTrustedCheckOptionPrompt }], &[yes]);
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
}

/// The displays' areas, for `move_within`.
#[cfg(target_os = "macos")]
pub fn displays() -> Vec<Area> {
    use objc2_core_graphics::{CGDisplayBounds, CGGetActiveDisplayList};
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        return Vec::new();
    }
    ids[..count as usize]
        .iter()
        .map(|&id| {
            let r = CGDisplayBounds(id);
            Area { x: r.origin.x, y: r.origin.y, w: r.size.width, h: r.size.height }
        })
        .collect()
}

/// Where the pointer is now, in global display points.
#[cfg(target_os = "macos")]
pub fn pointer_location() -> Option<(f64, f64)> {
    use objc2_core_graphics::CGEvent;
    let event = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&event));
    Some((p.x, p.y))
}

#[cfg(target_os = "macos")]
pub fn post(events: &[Ev]) -> anyhow::Result<()> {
    use anyhow::{bail, Context};
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventType, CGMouseButton, CGScrollEventUnit};

    if events.is_empty() {
        return Ok(());
    }
    // macOS silently drops events from apps the user hasn't allowed.
    if !allowed() {
        bail!("Caw Desktop is not allowed to control this Mac yet (System Settings → Privacy & Security → Accessibility)");
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
    let source = source.as_deref();
    let key = |code: u16, down: bool, flags: u64| {
        let event = CGEvent::new_keyboard_event(source, code, down).context("couldn't make a key event")?;
        CGEvent::set_flags(Some(&event), CGEventFlags(flags));
        Ok::<_, anyhow::Error>(event)
    };
    let mouse_button = |b: Button| match b {
        Button::Left => CGMouseButton::Left,
        Button::Right => CGMouseButton::Right,
        Button::Middle => CGMouseButton::Center,
    };
    for ev in events {
        match ev {
            Ev::Key { code, down, flags } => post_one(&*key(*code, *down, *flags)?),
            Ev::Modifier { code, flags } => {
                let event = key(*code, true, *flags)?;
                CGEvent::set_type(Some(&event), CGEventType::FlagsChanged);
                post_one(&event);
            }
            Ev::Text(units) => {
                for down in [true, false] {
                    let event = key(0, down, 0)?;
                    unsafe { CGEvent::keyboard_set_unicode_string(Some(&event), units.len() as _, units.as_ptr()) };
                    post_one(&event);
                }
                // Gives the focused app time to take each piece in order.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ev::Move { x, y, dx, dy, held } => {
                let (kind, button) = match held {
                    None => (CGEventType::MouseMoved, CGMouseButton::Left),
                    Some(Button::Left) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
                    Some(Button::Right) => (CGEventType::RightMouseDragged, CGMouseButton::Right),
                    Some(Button::Middle) => (CGEventType::OtherMouseDragged, CGMouseButton::Center),
                };
                let event = CGEvent::new_mouse_event(source, kind, CGPoint { x: *x, y: *y }, button).context("couldn't make a mouse event")?;
                CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventDeltaX, dx.round() as i64);
                CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventDeltaY, dy.round() as i64);
                post_one(&event);
            }
            Ev::Button { button, down, x, y, clicks } => {
                let kind = match (button, down) {
                    (Button::Left, true) => CGEventType::LeftMouseDown,
                    (Button::Left, false) => CGEventType::LeftMouseUp,
                    (Button::Right, true) => CGEventType::RightMouseDown,
                    (Button::Right, false) => CGEventType::RightMouseUp,
                    (Button::Middle, true) => CGEventType::OtherMouseDown,
                    (Button::Middle, false) => CGEventType::OtherMouseUp,
                };
                let event = CGEvent::new_mouse_event(source, kind, CGPoint { x: *x, y: *y }, mouse_button(*button)).context("couldn't make a mouse event")?;
                CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventClickState, *clicks);
                post_one(&event);
            }
            Ev::Scroll { vertical, horizontal } => {
                let event = CGEvent::new_scroll_wheel_event2(source, CGScrollEventUnit::Pixel, 2, *vertical, *horizontal, 0).context("couldn't make a scroll event")?;
                post_one(&event);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn post_one(event: &objc2_core_graphics::CGEvent) {
    use objc2_core_graphics::{CGEvent, CGEventTapLocation};
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_us_positions() {
        assert_eq!(keycode(Key::Char('a')), (0x00, 0));
        assert_eq!(keycode(Key::Char('t')), (0x11, 0));
        assert_eq!(keycode(Key::Char('z')), (0x06, 0));
        assert_eq!(keycode(Key::Char('0')), (0x1D, 0));
        assert_eq!(keycode(Key::Char('5')), (0x17, 0));
        assert_eq!(keycode(Key::Char('`')), (0x32, 0));
        assert_eq!(keycode(Key::Char('[')), (0x21, 0));
        assert_eq!(keycode(Key::Enter), (0x24, 0));
        assert_eq!(keycode(Key::Backspace), (0x33, 0));
        // Delete is the forward delete key; Backspace is the Mac's "delete".
        assert_eq!(keycode(Key::Delete), (0x75, FLAG_FN));
        assert_eq!(keycode(Key::Left), (0x7B, FLAG_NUMPAD | FLAG_FN));
        assert_eq!(keycode(Key::F(1)), (0x7A, FLAG_FN));
        assert_eq!(keycode(Key::F(12)), (0x6F, FLAG_FN));
        assert_eq!(keycode(Key::PageDown), (0x79, FLAG_FN));
    }

    #[test]
    fn every_protocol_key_has_its_own_code() {
        let mut names: Vec<String> = ["backspace", "delete", "enter", "tab", "escape", "left", "right", "up", "down", "home", "end", "space", "page_up", "page_down", "insert"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        names.extend((b'a'..=b'z').chain(b'0'..=b'9').map(|c| (c as char).to_string()));
        names.extend((1..=12).map(|n| format!("f{n}")));
        names.extend(crate::typer::PUNCTUATION.iter().map(|p| p.0.to_string()));
        let mut codes: Vec<u16> = names.iter().map(|n| keycode(Key::parse(n).unwrap()).0).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), names.len());
    }

    #[test]
    fn combos_hold_and_release_modifiers() {
        let k = |code, down, flags| Ev::Key { code, down, flags };
        assert_eq!(
            combo_events(Key::Char('t'), &[Mod::Super, Mod::Shift], 1),
            vec![
                Ev::Modifier { code: KVK_COMMAND, flags: FLAG_COMMAND },
                Ev::Modifier { code: KVK_SHIFT, flags: FLAG_COMMAND | FLAG_SHIFT },
                k(0x11, true, FLAG_COMMAND | FLAG_SHIFT),
                k(0x11, false, FLAG_COMMAND | FLAG_SHIFT),
                Ev::Modifier { code: KVK_SHIFT, flags: FLAG_COMMAND },
                Ev::Modifier { code: KVK_COMMAND, flags: 0 },
            ]
        );
        assert_eq!(combo_events(Key::Enter, &[], 2), vec![k(0x24, true, 0), k(0x24, false, 0), k(0x24, true, 0), k(0x24, false, 0)]);
        assert_eq!(combo_events(Key::Left, &[Mod::Ctrl], 1)[1], k(KVK_LEFT, true, FLAG_CONTROL | FLAG_NUMPAD | FLAG_FN));
        assert_eq!(modifier(Mod::Alt), (KVK_OPTION, FLAG_OPTION));
        assert_eq!(
            release_events(&[Mod::Ctrl, Mod::Alt]),
            vec![Ev::Modifier { code: KVK_OPTION, flags: FLAG_CONTROL }, Ev::Modifier { code: KVK_CONTROL, flags: 0 }]
        );
    }

    #[test]
    fn text_is_chunked_with_real_return() {
        let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert_eq!(text_events("hé\r\nok\t"), vec![Ev::Text(units("hé")), Ev::Key { code: 0x24, down: true, flags: 0 }, Ev::Key { code: 0x24, down: false, flags: 0 }, Ev::Text(units("ok")), Ev::Key { code: 0x30, down: true, flags: 0 }, Ev::Key { code: 0x30, down: false, flags: 0 }]);
        let long = "a".repeat(45);
        let chunks: Vec<usize> = text_events(&long).iter().map(|e| if let Ev::Text(u) = e { u.len() } else { 0 }).collect();
        assert_eq!(chunks, vec![20, 20, 5]);
        // A bird is two UTF-16 units and never split across events.
        let birds = format!("{}🐦", "a".repeat(19));
        assert_eq!(text_events(&birds), vec![Ev::Text(units(&"a".repeat(19))), Ev::Text(units("🐦"))]);
        assert!(text_events("").is_empty());
    }

    #[test]
    fn pointer_stays_on_the_displays() {
        let main = Area { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 };
        let right = Area { x: 1512.0, y: -200.0, w: 2560.0, h: 1440.0 };
        assert_eq!(move_within((100.0, 100.0), 10.0, -5.0, &[main]), (110.0, 95.0));
        assert_eq!(move_within((100.0, 100.0), -500.0, 2000.0, &[main]), (0.0, 981.0));
        // Across to the second display, and stopped at its top edge.
        assert_eq!(move_within((1500.0, 100.0), 50.0, 0.0, &[main, right]), (1550.0, 100.0));
        assert_eq!(move_within((1600.0, -150.0), 0.0, -100.0, &[main, right]), (1600.0, -200.0));
        assert_eq!(move_within((1.0, 1.0), 5.0, 5.0, &[]), (6.0, 6.0));
    }

    #[test]
    fn quick_second_click_is_a_double_click() {
        use std::time::{Duration, Instant};
        let (t, interval) = (Instant::now(), Duration::from_millis(500));
        let mut c = Clicks::default();
        assert_eq!(c.press(Button::Left, (10.0, 10.0), t, interval), 1);
        assert_eq!(c.press(Button::Left, (11.0, 10.0), t + Duration::from_millis(200), interval), 2);
        assert_eq!(c.current(), 2);
        assert_eq!(c.press(Button::Left, (11.0, 10.0), t + Duration::from_millis(400), interval), 3);
        assert_eq!(c.press(Button::Left, (11.0, 10.0), t + Duration::from_millis(1200), interval), 1);
        assert_eq!(c.press(Button::Right, (11.0, 10.0), t + Duration::from_millis(1300), interval), 1);
        assert_eq!(c.press(Button::Right, (60.0, 10.0), t + Duration::from_millis(1400), interval), 1);
    }
}
