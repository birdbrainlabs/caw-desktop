//! Moves the mouse. The input device lives on its own thread; commands go to
//! it over a channel. Linux uses whichever method `crate::linux` picked for
//! this desktop; Windows uses SendInput; macOS uses Quartz events.

use anyhow::{bail, Result};
use std::sync::mpsc::{channel, Sender};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) mod wayland;
#[cfg(target_os = "linux")]
use self::linux as backend;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as backend;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use self::macos as backend;

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod backend {
    use super::Cmd;
    use anyhow::{bail, Result};

    pub struct Device;

    impl Device {
        pub fn open() -> Result<Self> {
            bail!("mouse control is not supported on this system yet")
        }

        pub fn apply(&mut self, _: Cmd) -> Result<()> {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "left" => Self::Left,
            "right" => Self::Right,
            "middle" => Self::Middle,
            other => bail!("unknown button {other:?}"),
        })
    }
}

/// Linux evdev button codes (input-event-codes.h).
#[cfg(target_os = "linux")]
fn evdev_code(button: Button) -> u32 {
    match button {
        Button::Left => 0x110,
        Button::Right => 0x111,
        Button::Middle => 0x112,
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Cmd {
    Move { dx: f64, dy: f64 },
    Button { button: Button, pressed: bool },
    Scroll { dx: f64, dy: f64 },
}

/// Handle to the input thread.
#[derive(Clone)]
pub struct Pointer(Sender<Cmd>);

impl Pointer {
    pub fn start() -> Result<Self> {
        let mut device = backend::Device::open()?;
        let (tx, rx) = channel::<Cmd>();
        std::thread::spawn(move || {
            for cmd in rx {
                if let Err(e) = device.apply(cmd) {
                    eprintln!("caw-desktop: mouse control stopped: {e:#}");
                    break;
                }
            }
        });
        Ok(Self(tx))
    }

    pub fn send(&self, cmd: Cmd) -> Result<()> {
        #[cfg(target_os = "macos")]
        if !backend::available() {
            bail!("not allowed to control the mouse yet");
        }
        if self.0.send(cmd).is_err() {
            bail!("mouse control stopped");
        }
        Ok(())
    }
}

/// Carries the fractions left over when smooth movement is sent in whole units.
#[derive(Default)]
pub struct Accum(f64);

impl Accum {
    pub fn take(&mut self, v: f64) -> i32 {
        self.0 += v;
        let whole = self.0.trunc();
        self.0 -= whole;
        whole as i32
    }
}

/// Turns finger movement (dp) into pointer movement (px): faster swipes go
/// proportionally further, like a laptop touchpad.
pub fn accelerate(dx: f64, dy: f64, speed: f64, accel: f64) -> (f64, f64) {
    let magnitude = (dx * dx + dy * dy).sqrt();
    let gain = speed * (1.0 + accel * magnitude);
    (dx * gain, dy * gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceleration_grows_with_speed() {
        let (slow, _) = accelerate(1.0, 0.0, 1.5, 0.05);
        let (fast, _) = accelerate(20.0, 0.0, 1.5, 0.05);
        assert!((slow - 1.575).abs() < 1e-9);
        assert!(fast / 20.0 > slow);
        assert_eq!(accelerate(0.0, 0.0, 1.5, 0.05), (0.0, 0.0));
    }

    #[test]
    fn fractions_add_up() {
        let mut a = Accum::default();
        let total: i32 = (0..10).map(|_| a.take(0.35)).sum();
        assert_eq!(total, 3);
        assert_eq!(a.take(-1.6), -1);
    }
}
