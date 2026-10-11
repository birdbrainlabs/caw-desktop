//! Moves the mouse. Linux/Wayland uses the wlr virtual-pointer protocol,
//! which Hyprland (and other wlroots compositors) support directly.
//! One Wayland connection lives on its own thread; commands go to it over a channel.

use anyhow::{bail, Context, Result};
use std::sync::mpsc::{channel, Sender};
use std::time::Instant;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

// Linux input event codes (input-event-codes.h)
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Debug, Clone, Copy)]
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

    fn code(self) -> u32 {
        match self {
            Self::Left => BTN_LEFT,
            Self::Right => BTN_RIGHT,
            Self::Middle => BTN_MIDDLE,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Cmd {
    Move { dx: f64, dy: f64 },
    Button { button: Button, pressed: bool },
    Scroll { dx: f64, dy: f64 },
}

struct Noop;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Noop {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(Noop: ignore WlSeat);
delegate_noop!(Noop: ZwlrVirtualPointerManagerV1);
delegate_noop!(Noop: ZwlrVirtualPointerV1);

/// Handle to the virtual pointer thread.
#[derive(Clone)]
pub struct Pointer(Sender<Cmd>);

impl Pointer {
    pub fn start() -> Result<Self> {
        let conn = Connection::connect_to_env().context("no Wayland display")?;
        let (globals, queue) = registry_queue_init::<Noop>(&conn)?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).context("no wl_seat")?;
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).context("compositor has no virtual-pointer support")?;
        let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
        let smooth_scroll = pointer.version() >= 2;
        conn.flush()?;

        let (tx, rx) = channel::<Cmd>();
        std::thread::spawn(move || {
            let _keep = (queue, seat, manager);
            let start = Instant::now();
            for cmd in rx {
                let t = start.elapsed().as_millis() as u32;
                match cmd {
                    Cmd::Move { dx, dy } => pointer.motion(t, dx, dy),
                    Cmd::Button { button, pressed } => {
                        let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
                        pointer.button(t, button.code(), state);
                    }
                    Cmd::Scroll { dx, dy } => {
                        if smooth_scroll {
                            pointer.axis_source(AxisSource::Finger);
                        }
                        if dy != 0.0 {
                            pointer.axis(t, Axis::VerticalScroll, dy);
                        }
                        if dx != 0.0 {
                            pointer.axis(t, Axis::HorizontalScroll, dx);
                        }
                    }
                }
                pointer.frame();
                if conn.flush().is_err() {
                    eprintln!("caw-desktop: lost the Wayland connection; mouse control stopped");
                    break;
                }
            }
            pointer.destroy();
            let _ = conn.flush();
        });
        Ok(Self(tx))
    }

    pub fn send(&self, cmd: Cmd) -> Result<()> {
        self.0.send(cmd).context("mouse control stopped")
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
}
