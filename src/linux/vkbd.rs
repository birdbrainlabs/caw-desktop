//! Linux/Wayland: key combos through our own virtual keyboard, on wlroots
//! compositors. wtype uploads a tiny keymap of its own, so its keycodes mean
//! other keys to the compositor's shortcut matching, which uses the physical
//! keyboard's keymap (Super+Enter came out as Super+Escape on Hyprland). Here
//! the virtual keyboard gets a standard keymap with evdev keycodes, so a combo
//! means the same thing as on the physical keyboard.
//!
//! The seat hands out the keymap of whichever keyboard was used last, which is
//! wtype's after dictated text was typed. So the keymap is, in order: the
//! seat's, if it uses evdev keycodes (the user's own layout); the last such one
//! seen, remembered at service start or on an earlier combo; or one compiled
//! with libxkbcommon (loaded at runtime) for `XKB_DEFAULT_LAYOUT`, else "us".

use super::evdev;
use crate::typer::{Key, Mod};
use anyhow::{bail, Context, Result};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use xkbcommon_dl::{xkb_context_flags, xkb_keymap_compile_flags, xkb_keymap_format, xkb_rule_names};

/// The last keymap with evdev keycodes that the seat handed out (or that was
/// compiled), for when wtype's keyboard has since become the seat's current one.
static KEYMAP: Mutex<Option<String>> = Mutex::new(None);

fn saved_keymap() -> MutexGuard<'static, Option<String>> {
    KEYMAP.lock().unwrap_or_else(PoisonError::into_inner)
}

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
    /// Creates a virtual keyboard with an evdev keymap. Fails without pressing
    /// anything if there is no such keymap or no virtual-keyboard support.
    pub fn open() -> Result<Self> {
        let conn = Connection::connect_to_env().context("no Wayland display")?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).context("no wl_seat")?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).context("compositor has no virtual-keyboard support")?;
        let keymap = evdev_keymap(seat_keymap(&mut queue, &seat))?;
        let file = keymap_file(&keymap)?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        keyboard.keymap(KeymapFormat::XkbV1.into(), file.as_fd(), keymap.len() as u32 + 1);
        let mut state = State::default();
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

/// Remembers the seat's keymap if it uses evdev keycodes. Called at service
/// start, before wtype has typed anything, so combos keep the user's layout.
pub fn remember_seat_keymap() -> Result<()> {
    let conn = Connection::connect_to_env().context("no Wayland display")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let seat: WlSeat = globals.bind(&queue.handle(), 1..=7, ()).context("no wl_seat")?;
    let keymap = seat_keymap(&mut queue, &seat)?;
    check_keycodes(&keymap)?;
    *saved_keymap() = Some(keymap);
    Ok(())
}

/// Asks the seat for a keyboard just long enough to read its current keymap.
fn seat_keymap(queue: &mut EventQueue<State>, seat: &WlSeat) -> Result<String> {
    let seat_keyboard = seat.get_keyboard(&queue.handle(), ());
    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    if seat_keyboard.version() >= 3 {
        seat_keyboard.release();
    }
    let (fd, size) = state.keymap.take().context("the seat sent no keyboard keymap")?;
    let mut text = vec![0; size as usize];
    File::from(fd).read_exact_at(&mut text, 0).context("reading the seat's keymap")?;
    Ok(String::from_utf8_lossy(&text).trim_end_matches('\0').to_owned())
}

/// The seat's keymap if it uses evdev keycodes (and remembers it), else the
/// remembered one, else a standard one compiled here.
fn evdev_keymap(seat: Result<String>) -> Result<String> {
    let mut saved = saved_keymap();
    let why = match seat.and_then(|keymap| check_keycodes(&keymap).map(|()| keymap)) {
        Ok(keymap) => {
            *saved = Some(keymap.clone());
            return Ok(keymap);
        }
        Err(why) => why,
    };
    if let Some(keymap) = saved.as_ref() {
        return Ok(keymap.clone());
    }
    let keymap = compile_keymap().and_then(|keymap| check_keycodes(&keymap).map(|()| keymap));
    let keymap = keymap.map_err(|e| e.context(format!("{why:#}, and there is no standard one either")))?;
    *saved = Some(keymap.clone());
    Ok(keymap)
}

/// Compiles a standard evdev keymap for `XKB_DEFAULT_LAYOUT` (with its
/// `_VARIANT` and `_OPTIONS`), else "us". libxkbcommon is loaded at runtime,
/// so building needs no extra packages.
fn compile_keymap() -> Result<String> {
    let xkb = xkbcommon_dl::xkbcommon_option().context("libxkbcommon couldn't be loaded")?;
    let name = |var, default: &str| CString::new(std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.into()));
    let (layout, variant, options) = (name("XKB_DEFAULT_LAYOUT", "us")?, name("XKB_DEFAULT_VARIANT", "")?, name("XKB_DEFAULT_OPTIONS", "")?);
    let names = xkb_rule_names {
        rules: c"evdev".as_ptr(),
        model: c"pc105".as_ptr(),
        layout: layout.as_ptr(),
        variant: variant.as_ptr(),
        options: options.as_ptr(),
    };
    // SAFETY: plain libxkbcommon calls; every pointer is checked before use,
    // and the keymap text is copied before it is freed.
    unsafe {
        let context = (xkb.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_ENVIRONMENT_NAMES);
        if context.is_null() {
            bail!("libxkbcommon couldn't start");
        }
        let keymap = (xkb.xkb_keymap_new_from_names)(context, &names, xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS);
        (xkb.xkb_context_unref)(context);
        if keymap.is_null() {
            bail!("libxkbcommon has no keymap for layout {layout:?}");
        }
        let text = (xkb.xkb_keymap_get_as_string)(keymap, xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1);
        (xkb.xkb_keymap_unref)(keymap);
        if text.is_null() {
            bail!("libxkbcommon couldn't write out the keymap");
        }
        let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
        libc::free(text.cast_mut().cast());
        Ok(owned)
    }
}

/// Shares the keymap with the compositor as an anonymous file, NUL-terminated
/// as the protocol expects.
fn keymap_file(keymap: &str) -> Result<File> {
    // SAFETY: memfd_create takes a NUL-terminated name and returns a new fd we own.
    let fd = unsafe { libc::memfd_create(c"caw-keymap".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("creating the keymap file");
    }
    // SAFETY: fd is a fresh, valid file descriptor nothing else owns.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(keymap.as_bytes()).and_then(|()| file.write_all(b"\0")).context("writing the keymap file")?;
    Ok(file)
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
fn check_keycodes(keymap: &str) -> Result<()> {
    let text: String = keymap.split_whitespace().collect();
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

    #[test]
    fn keycode_check() {
        assert!(check_keycodes("<RTRN> = 36;\n<LWIN>   = 133;").is_ok());
        assert!(check_keycodes("<K1> = 9;\n<K2> = 10;").is_err());
    }

    /// Needs libxkbcommon and its keymap data, which every Wayland desktop has.
    #[test]
    fn compiled_keymap_uses_evdev_keycodes() {
        if xkbcommon_dl::xkbcommon_option().is_none() {
            eprintln!("skipped: no libxkbcommon");
            return;
        }
        check_keycodes(&compile_keymap().unwrap()).unwrap();
    }
}
