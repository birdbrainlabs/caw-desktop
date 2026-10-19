//! macOS: Quartz mouse events. Moves are relative to where the pointer is now
//! and stop at the edges of the displays.

use super::{Accum, Button, Cmd};
use crate::cgevent::{self, Clicks, Ev};
use anyhow::Result;
use std::time::{Duration, Instant};

/// Pixels scrolled per dp of finger movement, as on Wayland.
const PIXELS_PER_DP: f64 = 1.0;

#[derive(Default)]
pub struct Device {
    held: Option<Button>,
    clicks: Clicks,
    wheel_y: Accum,
    wheel_x: Accum,
}

/// The pointer is only usable once the user allows Caw Desktop in
/// Privacy & Security → Accessibility; until then the phone gets `pointer_unavailable`.
pub fn available() -> bool {
    cgevent::allowed()
}

impl Device {
    pub fn open() -> Result<Self> {
        Ok(Self::default())
    }

    pub fn apply(&mut self, cmd: Cmd) -> Result<()> {
        let at = cgevent::pointer_location().unwrap_or_default();
        let events = match cmd {
            Cmd::Move { dx, dy } => {
                let (x, y) = cgevent::move_within(at, dx, dy, &cgevent::displays());
                vec![Ev::Move { x, y, dx: x - at.0, dy: y - at.1, held: self.held }]
            }
            Cmd::Button { button, pressed: true } => {
                self.held = Some(button);
                let clicks = self.clicks.press(button, at, Instant::now(), double_click_interval());
                vec![Ev::Button { button, down: true, x: at.0, y: at.1, clicks }]
            }
            Cmd::Button { button, pressed: false } => {
                if self.held == Some(button) {
                    self.held = None;
                }
                vec![Ev::Button { button, down: false, x: at.0, y: at.1, clicks: self.clicks.current() }]
            }
            // The protocol's positive dy scrolls down; macOS's positive wheel scrolls up (and left).
            Cmd::Scroll { dx, dy } => {
                let (vertical, horizontal) = (self.wheel_y.take(-dy * PIXELS_PER_DP), self.wheel_x.take(-dx * PIXELS_PER_DP));
                if vertical == 0 && horizontal == 0 {
                    return Ok(());
                }
                vec![Ev::Scroll { vertical, horizontal }]
            }
        };
        // Not allowed yet (or a refused event) only affects this one command.
        if let Err(e) = cgevent::post(&events) {
            eprintln!("caw-desktop: {e:#}");
        }
        Ok(())
    }
}

/// The double-click speed set in System Settings.
fn double_click_interval() -> Duration {
    let seconds = objc2_app_kit::NSEvent::doubleClickInterval();
    Duration::try_from_secs_f64(seconds).unwrap_or(Duration::from_millis(500))
}
