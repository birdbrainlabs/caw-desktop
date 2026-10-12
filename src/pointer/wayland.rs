//! Linux/Wayland: the wlr virtual-pointer protocol, which Hyprland (and other
//! wlroots compositors) support directly.

use super::{Button, Cmd};
use anyhow::{Context, Result};
use std::time::Instant;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

// Linux input event codes (input-event-codes.h)
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

struct Noop;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Noop {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(Noop: ignore WlSeat);
delegate_noop!(Noop: ZwlrVirtualPointerManagerV1);
delegate_noop!(Noop: ZwlrVirtualPointerV1);

pub struct Device {
    conn: Connection,
    _keep: (EventQueue<Noop>, WlSeat, ZwlrVirtualPointerManagerV1),
    pointer: ZwlrVirtualPointerV1,
    smooth_scroll: bool,
    start: Instant,
}

impl Device {
    pub fn open() -> Result<Self> {
        let conn = Connection::connect_to_env().context("no Wayland display")?;
        let (globals, queue) = registry_queue_init::<Noop>(&conn)?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).context("no wl_seat")?;
        let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).context("compositor has no virtual-pointer support")?;
        let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());
        let smooth_scroll = pointer.version() >= 2;
        conn.flush()?;
        Ok(Self { conn, _keep: (queue, seat, manager), pointer, smooth_scroll, start: Instant::now() })
    }

    pub fn apply(&mut self, cmd: Cmd) -> Result<()> {
        let t = self.start.elapsed().as_millis() as u32;
        match cmd {
            Cmd::Move { dx, dy } => self.pointer.motion(t, dx, dy),
            Cmd::Button { button, pressed } => {
                let code = match button {
                    Button::Left => BTN_LEFT,
                    Button::Right => BTN_RIGHT,
                    Button::Middle => BTN_MIDDLE,
                };
                let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
                self.pointer.button(t, code, state);
            }
            Cmd::Scroll { dx, dy } => {
                if self.smooth_scroll {
                    self.pointer.axis_source(AxisSource::Finger);
                }
                if dy != 0.0 {
                    self.pointer.axis(t, Axis::VerticalScroll, dy);
                }
                if dx != 0.0 {
                    self.pointer.axis(t, Axis::HorizontalScroll, dx);
                }
            }
        }
        self.pointer.frame();
        self.conn.flush().context("lost the Wayland connection")?;
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.pointer.destroy();
        let _ = self.conn.flush();
    }
}
