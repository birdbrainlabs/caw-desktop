//! Linux/Wayland: key combos through our own virtual keyboard, on wlroots
//! compositors. wtype uploads a tiny keymap of its own, so its keycodes mean
//! other keys to the compositor's shortcut matching, which uses the physical
//! keyboard's keymap (Super+Enter came out as Super+Escape on Hyprland). Here
//! the virtual keyboard gets the seat's own keymap and evdev keycodes, so a
//! combo means the same thing as on the physical keyboard.

use super::evdev;
use crate::typer::{Key, Mod};
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::time::Instant;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

/// Collects the seat's keymap (an XKB text file shared as an fd).
#[derive(Default)]
struct State {
    keymap: Option<(OwnedFd, u32)>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Keymap { format: WEnum::Value(KeymapFormat::XkbV1), fd, size } = event {
            state.keymap = Some((fd, size));
        }
    }
}
delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);

pub struct Keyboard {
    queue: EventQueue<State>,
    state: State,
    keyboard: ZwpVirtualKeyboardV1,
    start: Instant,
}

impl Keyboard {
    /// Creates a virtual keyboard with the seat's keymap. Fails without
    /// pressing anything if there is no keymap or no virtual-keyboard support.
    pub fn open() -> Result<Self> {
        let conn = Connection::connect_to_env().context("no Wayland display")?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).context("no wl_seat")?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).context("compositor has no virtual-keyboard support")?;
        let seat_keyboard = seat.get_keyboard(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state)?;
        if seat_keyboard.version() >= 3 {
            seat_keyboard.release();
        }
        let (fd, size) = state.keymap.take().context("the seat sent no keyboard keymap")?;
        let keymap = File::from(fd);
        check_keycodes(&keymap, size)?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        keyboard.keymap(KeymapFormat::XkbV1.into(), keymap.as_fd(), size);
        queue.roundtrip(&mut state).context("lost the Wayland connection")?;
        Ok(Self { queue, state, keyboard, start: Instant::now() })
    }

    /// Holds `mods`, taps `key` `count` times, then lets go of the modifiers
    /// in reverse order, even if a tap failed.
    pub fn press(&mut self, key: Key, mods: &[Mod], count: u32) -> Result<()> {
        let mut held = 0;
        let mut result = Ok(());
        for &m in mods {
            result = self.key(evdev::for_mod(m), KeyState::Pressed, mask(&mods[..=held]));
            if result.is_err() {
                break;
            }
            held += 1;
        }
        if result.is_ok() {
            let code = evdev::for_key(key);
            let depressed = mask(mods);
            for _ in 0..count {
                result = self.key(code, KeyState::Pressed, depressed).and_then(|()| self.key(code, KeyState::Released, depressed));
                if result.is_err() {
                    break;
                }
            }
        }
        for i in (0..held).rev() {
            let released = self.key(evdev::for_mod(mods[i]), KeyState::Released, mask(&mods[..i]));
            result = result.and(released);
        }
        result
    }

    /// Sends one key event and the modifiers held after it, and waits until
    /// the compositor has them.
    fn key(&mut self, code: u32, state: KeyState, depressed: u32) -> Result<()> {
        let time = self.start.elapsed().as_millis() as u32;
        self.keyboard.key(time, code, state.into());
        self.keyboard.modifiers(depressed, 0, 0, 0);
        self.queue.roundtrip(&mut self.state).context("lost the Wayland connection")?;
        Ok(())
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.keyboard.destroy();
        let _ = self.queue.roundtrip(&mut self.state);
    }
}

/// The modifier mask of a standard (evdev, "pc") keymap: Super is Mod4, Alt is Mod1.
fn mask(mods: &[Mod]) -> u32 {
    mods.iter()
        .map(|m| match m {
            Mod::Shift => 1,
            Mod::Ctrl => 4,
            Mod::Alt => 8,
            Mod::Super => 64,
        })
        .fold(0, |a, b| a | b)
}

/// Refuses a keymap that doesn't use evdev keycodes (Enter = 36, left Super
/// = 133), such as wtype's own if its virtual keyboard was the last one used.
fn check_keycodes(keymap: &File, size: u32) -> Result<()> {
    let mut text = vec![0; size as usize];
    keymap.read_exact_at(&mut text, 0).context("reading the seat's keymap")?;
    let text: String = String::from_utf8_lossy(&text).split_whitespace().collect();
    if !(text.contains("<RTRN>=36;") && text.contains("<LWIN>=133;")) {
        bail!("the seat's keymap doesn't use evdev keycodes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert_eq!(mask(&[]), 0);
        assert_eq!(mask(&[Mod::Super]), 64);
        assert_eq!(mask(&[Mod::Ctrl, Mod::Shift]), 5);
        assert_eq!(mask(&[Mod::Super, Mod::Ctrl, Mod::Alt, Mod::Shift]), 77);
    }
}
