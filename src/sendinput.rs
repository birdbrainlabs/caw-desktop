//! Windows keyboard and mouse input through SendInput. The event lists are
//! built by plain functions, tested on every platform; only `send` touches
//! Windows.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::pointer::Button;
use crate::typer::Key;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ev {
    /// A virtual key such as Backspace.
    Key { vk: u16, extended: bool, up: bool },
    /// One UTF-16 code unit, typed as-is whatever the keyboard layout.
    Unicode { unit: u16, up: bool },
    Move { dx: i32, dy: i32 },
    Button { button: Button, pressed: bool },
    /// In wheel units; 120 is one notch. Positive scrolls up (or right if horizontal).
    Wheel { amount: i32, horizontal: bool },
}

// Virtual-key codes (winuser.h)
const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_DELETE: u16 = 0x2E;

/// The virtual key, and whether it is an "extended" key (the navigation
/// cluster), which Windows otherwise reads as the number pad.
fn vk(key: Key) -> (u16, bool) {
    match key {
        Key::Backspace => (VK_BACK, false),
        Key::Delete => (VK_DELETE, true),
        Key::Enter => (VK_RETURN, false),
        Key::Tab => (VK_TAB, false),
        Key::Escape => (VK_ESCAPE, false),
        Key::Left => (VK_LEFT, true),
        Key::Right => (VK_RIGHT, true),
        Key::Up => (VK_UP, true),
        Key::Down => (VK_DOWN, true),
        Key::Home => (VK_HOME, true),
        Key::End => (VK_END, true),
    }
}

fn tap(vk: u16, extended: bool) -> [Ev; 2] {
    [Ev::Key { vk, extended, up: false }, Ev::Key { vk, extended, up: true }]
}

pub fn key_events(key: Key, count: u32) -> Vec<Ev> {
    let (vk, extended) = vk(key);
    (0..count).flat_map(|_| tap(vk, extended)).collect()
}

/// Newlines and tabs become real Enter and Tab presses (apps ignore them as
/// Unicode characters); everything else is typed as Unicode.
pub fn text_events(text: &str) -> Vec<Ev> {
    let mut events = Vec::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => events.extend(tap(VK_RETURN, false)),
            '\t' => events.extend(tap(VK_TAB, false)),
            c => {
                for &unit in c.encode_utf16(&mut [0; 2]).iter() {
                    events.push(Ev::Unicode { unit, up: false });
                    events.push(Ev::Unicode { unit, up: true });
                }
            }
        }
    }
    events
}

#[cfg(windows)]
pub fn send(events: &[Ev]) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT};
    if events.is_empty() {
        return Ok(());
    }
    let inputs: Vec<INPUT> = events.iter().map(to_input).collect();
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        // Windows refuses input into apps running as administrator unless we are too.
        anyhow::bail!("Windows blocked the input ({sent} of {} events sent); is the focused app running as administrator?", inputs.len());
    }
    Ok(())
}

#[cfg(windows)]
fn to_input(ev: &Ev) -> windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    let keyboard = |vk: u16, scan: u16, flags: u32| KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 };
    let mouse = |dx: i32, dy: i32, data: i32, flags: u32| MOUSEINPUT { dx, dy, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 };
    let up = |up: bool| if up { KEYEVENTF_KEYUP } else { 0 };
    match *ev {
        Ev::Key { vk, extended, up: is_up } => {
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki = keyboard(vk, 0, up(is_up) | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 });
        }
        Ev::Unicode { unit, up: is_up } => {
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki = keyboard(0, unit, KEYEVENTF_UNICODE | up(is_up));
        }
        Ev::Move { dx, dy } => {
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi = mouse(dx, dy, 0, MOUSEEVENTF_MOVE);
        }
        Ev::Button { button, pressed } => {
            let flags = match (button, pressed) {
                (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (Button::Left, false) => MOUSEEVENTF_LEFTUP,
                (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
                (Button::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (Button::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi = mouse(0, 0, 0, flags);
        }
        Ev::Wheel { amount, horizontal } => {
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi = mouse(0, 0, amount, if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL });
        }
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_becomes_unicode_with_real_enter() {
        let ev = text_events("hé\r\n");
        assert_eq!(
            ev,
            vec![
                Ev::Unicode { unit: 'h' as u16, up: false },
                Ev::Unicode { unit: 'h' as u16, up: true },
                Ev::Unicode { unit: 0xE9, up: false },
                Ev::Unicode { unit: 0xE9, up: true },
                Ev::Key { vk: VK_RETURN, extended: false, up: false },
                Ev::Key { vk: VK_RETURN, extended: false, up: true },
            ]
        );
        // Characters outside the BMP are two UTF-16 units.
        assert_eq!(text_events("🐦").len(), 4);
    }

    #[test]
    fn keys_repeat_and_navigation_keys_are_extended() {
        let ev = key_events(Key::Backspace, 3);
        assert_eq!(ev.len(), 6);
        assert_eq!(ev[0], Ev::Key { vk: VK_BACK, extended: false, up: false });
        assert_eq!(key_events(Key::Left, 1)[0], Ev::Key { vk: VK_LEFT, extended: true, up: false });
    }
}

/// Run on a Windows desktop with `cargo test -- --ignored` (CI does this).
#[cfg(all(test, windows))]
mod desktop_tests {
    use super::*;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

    #[test]
    #[ignore = "moves the real mouse; needs a Windows desktop"]
    fn moves_the_cursor() {
        let mut p = POINT { x: 0, y: 0 };
        unsafe { SetCursorPos(200, 200) };
        send(&[Ev::Move { dx: 40, dy: 25 }]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        unsafe { GetCursorPos(&mut p) };
        assert!(p.x > 200 && p.y > 200, "cursor at {},{}", p.x, p.y);
    }

    #[test]
    #[ignore = "types into a real window; needs a Windows desktop"]
    fn types_into_a_text_box() {
        use std::ptr::null_mut;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (class, title) = (wide("EDIT"), wide(""));
        let edit = unsafe {
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW | WS_VISIBLE | ES_MULTILINE as u32, 100, 100, 600, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        assert!(!edit.is_null(), "could not open a text box");
        let pump = || {
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            for _ in 0..30 {
                while unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        unsafe {
            SetForegroundWindow(edit);
            SetFocus(edit);
        }
        pump();

        let mut events = text_events("Caw é 🐦\nline two");
        events.extend(key_events(Key::Backspace, 1));
        send(&events).unwrap();
        pump();

        let mut buf = [0u16; 256];
        let len = unsafe { GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) };
        let typed = String::from_utf16_lossy(&buf[..len as usize]);
        unsafe { DestroyWindow(edit) };
        assert_eq!(typed, "Caw é 🐦\r\nline tw");
    }
}
