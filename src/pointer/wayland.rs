//! Linux/Wayland: the wlr virtual-pointer protocol, which Hyprland (and other
//! wlroots compositors) support directly.

use super::{evdev_code, Cmd};
use anyhow::{Context, Result};
use std::time::Instant;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

struct Noop;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Noop {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(Noop: ignore WlSeat);
delegate_noop!(Noop: ZwlrVirtualPointerManagerV1);
delegate_noop!(Noop: ZwlrVirtualPointerV1);

/// True if the compositor offers both a virtual keyboard (used by wtype) and
/// a virtual pointer, as wlroots compositors do. GNOME and KDE don't.
pub fn has_virtual_input() -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, _)) = registry_queue_init::<Noop>(&conn) else { return false };
    globals.contents().with_list(|list| {
        let has = |name: &str| list.iter().any(|g| g.interface == name);
        has("zwp_virtual_keyboard_manager_v1") && has(ZwlrVirtualPointerManagerV1::interface().name)
    })
}

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
                let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
                self.pointer.button(t, evdev_code(button), state);
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
