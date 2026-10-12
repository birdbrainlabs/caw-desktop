//! Linux: moves the mouse with whichever method `crate::linux` picked.

use super::{evdev_code, wayland, Accum, Button, Cmd};
use crate::linux::{self, Input, Portal, X11};
use anyhow::{bail, Result};
use tokio::runtime::Handle;

/// dp of finger scroll per X11 wheel click; Wayland treats about 15 as one notch.
const DP_PER_CLICK: f64 = 15.0;

pub enum Device {
    Wlr(wayland::Device),
    /// The portal is async; this thread waits on the receiver's runtime for each call.
    Portal(&'static Portal, Handle),
    X11 { x11: &'static X11, x: Accum, y: Accum, wheel_x: Accum, wheel_y: Accum },
}

impl Device {
    pub fn open() -> Result<Self> {
        Ok(match linux::get() {
            Input::Wlr => Self::Wlr(wayland::Device::open()?),
            Input::Portal(portal) => Self::Portal(portal, Handle::current()),
            Input::X11(x11) => Self::X11 { x11: x11.as_ref(), x: Accum::default(), y: Accum::default(), wheel_x: Accum::default(), wheel_y: Accum::default() },
            Input::Unavailable(why) => bail!("{why}"),
        })
    }

    pub fn apply(&mut self, cmd: Cmd) -> Result<()> {
        match self {
            Self::Wlr(device) => device.apply(cmd),
            Self::Portal(portal, rt) => {
                // A refused call (e.g. the user revoked access) shouldn't stop later ones.
                let result = rt.block_on(async {
                    match cmd {
                        Cmd::Move { dx, dy } => portal.motion(dx, dy).await,
                        Cmd::Button { button, pressed } => portal.button(evdev_code(button), pressed).await,
                        Cmd::Scroll { dx, dy } => portal.scroll(dx, dy).await,
                    }
                });
                if let Err(e) = result {
                    eprintln!("caw-desktop: mouse: {e:#}");
                }
                Ok(())
            }
            Self::X11 { x11, x, y, wheel_x, wheel_y } => {
                match cmd {
                    Cmd::Move { dx, dy } => {
                        let (dx, dy) = (x.take(dx), y.take(dy));
                        if dx != 0 || dy != 0 {
                            x11.motion(dx.clamp(-32768, 32767) as i16, dy.clamp(-32768, 32767) as i16)?;
                        }
                    }
                    Cmd::Button { button, pressed } => {
                        let number = match button {
                            Button::Left => 1,
                            Button::Middle => 2,
                            Button::Right => 3,
                        };
                        x11.button(number, pressed)?;
                    }
                    Cmd::Scroll { dx, dy } => {
                        for (clicks, back, forward) in [(wheel_y.take(dy / DP_PER_CLICK), 4, 5), (wheel_x.take(dx / DP_PER_CLICK), 6, 7)] {
                            let button = if clicks < 0 { back } else { forward };
                            for _ in 0..clicks.unsigned_abs().min(20) {
                                x11.button(button, true)?;
                                x11.button(button, false)?;
                            }
                        }
                    }
                }
                Ok(())
            }
        }
    }
}
