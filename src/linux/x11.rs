//! X11 desktops (Linux Mint Cinnamon, XFCE, MATE, Xorg sessions): the XTEST
//! extension fakes keyboard and mouse input with no setup or permission.

use anyhow::{bail, Context, Result};
use std::sync::Mutex;
use std::time::Duration;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{ConnectionExt as _, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use super::keysym::SHIFT_L;

pub struct X11 {
    conn: RustConnection,
    /// Typing is one keymap edit at a time.
    typing: Mutex<()>,
}

/// The keyboard layout: which keysyms each keycode produces.
struct Keymap {
    first: u8,
    per_code: usize,
    keysyms: Vec<u32>,
}

impl Keymap {
    /// The keycode for a keysym, and whether Shift is needed.
    fn find(&self, keysym: u32) -> Option<(u8, bool)> {
        self.keysyms.chunks(self.per_code).enumerate().find_map(|(i, syms)| {
            let level = syms.iter().take(2).position(|&s| s == keysym)?;
            Some((self.first + i as u8, level == 1))
        })
    }

    /// A keycode with nothing on it, borrowed for keysyms the layout lacks.
    fn spare(&self) -> Option<u8> {
        self.keysyms.chunks(self.per_code).rposition(|syms| syms.iter().all(|&s| s == 0)).map(|i| self.first + i as u8)
    }
}

impl X11 {
    pub fn open() -> Result<Self> {
        let (conn, _) = x11rb::connect(None).context("no X11 display")?;
        conn.extension_information(xtest::X11_EXTENSION_NAME)?.context("the X server has no XTEST extension")?;
        Ok(Self { conn, typing: Mutex::new(()) })
    }

    fn keymap(&self) -> Result<Keymap> {
        let setup = self.conn.setup();
        let (first, last) = (setup.min_keycode, setup.max_keycode);
        let reply = self.conn.get_keyboard_mapping(first, last - first + 1)?.reply()?;
        Ok(Keymap { first, per_code: reply.keysyms_per_keycode as usize, keysyms: reply.keysyms })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<()> {
        self.conn.xtest_fake_input(kind, detail, 0, x11rb::NONE, x, y, 0)?;
        Ok(())
    }

    /// Waits until the X server has handled everything sent so far.
    fn sync(&self) -> Result<()> {
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }

    fn tap(&self, code: u8, shift: Option<u8>) -> Result<()> {
        if let Some(shift) = shift {
            self.fake(KEY_PRESS_EVENT, shift, 0, 0)?;
        }
        self.fake(KEY_PRESS_EVENT, code, 0, 0)?;
        self.fake(KEY_RELEASE_EVENT, code, 0, 0)?;
        if let Some(shift) = shift {
            self.fake(KEY_RELEASE_EVENT, shift, 0, 0)?;
        }
        Ok(())
    }

    pub fn tap_keysyms(&self, keysyms: &[u32]) -> Result<()> {
        let _turn = self.typing.lock().unwrap();
        // Read the layout each time: the user may have switched it.
        let map = self.keymap()?;
        let shift = map.find(SHIFT_L).map(|(code, _)| code);
        let spare = map.spare();
        let mut borrowed = false;
        for &sym in keysyms {
            if let Some((code, shifted)) = map.find(sym) {
                self.tap(code, shifted.then_some(shift).flatten())?;
                continue;
            }
            // Not on this layout (e.g. "é" on a US keyboard): put it on the spare key, like xdotool does.
            let Some(spare) = spare else { bail!("the keyboard layout has no room for keysym {sym:#x}") };
            self.conn.change_keyboard_mapping(1, spare, map.per_code as u8, &vec![sym; map.per_code])?;
            self.sync()?;
            // Apps must see the new mapping before the key, and the key before the next change.
            std::thread::sleep(Duration::from_millis(25));
            self.tap(spare, None)?;
            self.sync()?;
            std::thread::sleep(Duration::from_millis(25));
            borrowed = true;
        }
        if let (true, Some(spare)) = (borrowed, spare) {
            self.conn.change_keyboard_mapping(1, spare, map.per_code as u8, &vec![0; map.per_code])?;
        }
        self.sync()
    }

    pub fn motion(&self, dx: i16, dy: i16) -> Result<()> {
        // detail 1: relative to the current position.
        self.fake(MOTION_NOTIFY_EVENT, 1, dx, dy)?;
        self.conn.flush()?;
        Ok(())
    }

    /// X button numbers: 1 left, 2 middle, 3 right, 4–7 scroll up/down/left/right.
    pub fn button(&self, button: u8, pressed: bool) -> Result<()> {
        self.fake(if pressed { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, button, 0, 0)?;
        self.conn.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_keys_and_shift_level() {
        // Two keycodes from 10: "a"/"A", and an empty one.
        let map = Keymap { first: 10, per_code: 2, keysyms: vec![0x61, 0x41, 0, 0] };
        assert_eq!(map.find(0x61), Some((10, false)));
        assert_eq!(map.find(0x41), Some((10, true)));
        assert_eq!(map.find(0xe9), None);
        assert_eq!(map.spare(), Some(11));
    }
}

/// Run with an X display: `cargo test -- --ignored` (CI uses Xvfb).
#[cfg(test)]
mod desktop_tests {
    use super::*;
    use crate::linux::keysym;
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::Event;

    #[test]
    #[ignore = "types into a real window; needs an X11 display"]
    fn types_into_an_x11_window() {
        let input = X11::open().unwrap();
        // A separate connection owns the window, like a real app.
        let (app, screen) = x11rb::connect(None).unwrap();
        let root = app.setup().roots[screen].root;
        let win = app.generate_id().unwrap();
        let events = EventMask::KEY_PRESS | EventMask::EXPOSURE;
        app.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().event_mask(events)).unwrap();
        app.map_window(win).unwrap();
        app.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        app.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME).unwrap();
        app.get_input_focus().unwrap().reply().unwrap();

        // Read keys as they arrive, following keymap changes as apps do.
        let reader = std::thread::spawn(move || {
            let load = |app: &RustConnection| {
                let s = app.setup();
                let r = app.get_keyboard_mapping(s.min_keycode, s.max_keycode - s.min_keycode + 1).unwrap().reply().unwrap();
                (s.min_keycode, r.keysyms_per_keycode as usize, r.keysyms)
            };
            let (mut first, mut per, mut syms) = load(&app);
            let mut typed = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while typed.len() < 4 && std::time::Instant::now() < deadline {
                match app.poll_for_event().unwrap() {
                    Some(Event::MappingNotify(_)) => (first, per, syms) = load(&app),
                    Some(Event::KeyPress(k)) => {
                        let row = &syms[(k.detail - first) as usize * per..][..per];
                        let shifted = u16::from(k.state) & u16::from(KeyButMask::SHIFT) != 0;
                        let sym = if shifted && row[1] != 0 { row[1] } else { row[0] };
                        if sym != SHIFT_L {
                            typed.push(sym);
                        }
                    }
                    Some(_) => {}
                    None => std::thread::sleep(Duration::from_millis(2)),
                }
            }
            typed
        });
        std::thread::sleep(Duration::from_millis(100));
        input.tap_keysyms(&keysym::for_text("Hi é")).unwrap();
        assert_eq!(reader.join().unwrap(), vec![0x48, 0x69, 0x20, 0xe9]);
    }

    #[test]
    #[ignore = "moves the real mouse; needs an X11 display"]
    fn moves_the_pointer() {
        let input = X11::open().unwrap();
        let (app, screen) = x11rb::connect(None).unwrap();
        let root = app.setup().roots[screen].root;
        app.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, 200, 200).unwrap();
        app.get_input_focus().unwrap().reply().unwrap();
        input.motion(40, 25).unwrap();
        input.sync().unwrap();
        let p = app.query_pointer(root).unwrap().reply().unwrap();
        assert_eq!((p.root_x, p.root_y), (240, 225));
    }
}
