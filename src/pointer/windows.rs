//! Windows: SendInput mouse events.

use super::{Accum, Cmd};
use crate::sendinput::{self, Ev};
use anyhow::Result;

/// Wheel units per dp of finger scroll. 120 is one notch; Linux treats about
/// 15 as one notch, so this keeps scrolling the same distance on both.
const WHEEL_PER_DP: f64 = 8.0;

#[derive(Default)]
pub struct Device {
    x: Accum,
    y: Accum,
    wheel_y: Accum,
    wheel_x: Accum,
}

impl Device {
    pub fn open() -> Result<Self> {
        Ok(Self::default())
    }

    pub fn apply(&mut self, cmd: Cmd) -> Result<()> {
        let events = match cmd {
            Cmd::Move { dx, dy } => vec![Ev::Move { dx: self.x.take(dx), dy: self.y.take(dy) }],
            Cmd::Button { button, pressed } => vec![Ev::Button { button, pressed }],
            // The protocol's positive dy scrolls down; Windows' positive wheel scrolls up.
            Cmd::Scroll { dx, dy } => vec![
                Ev::Wheel { amount: self.wheel_y.take(-dy * WHEEL_PER_DP), horizontal: false },
                Ev::Wheel { amount: self.wheel_x.take(dx * WHEEL_PER_DP), horizontal: true },
            ],
        };
        let events: Vec<Ev> = events.into_iter().filter(|e| !matches!(e, Ev::Move { dx: 0, dy: 0 } | Ev::Wheel { amount: 0, .. })).collect();
        // A refusal (an administrator window is focused) only affects that one
        // event, so keep the mouse working for the next one.
        if let Err(e) = sendinput::send(&events) {
            eprintln!("caw-desktop: {e:#}");
        }
        Ok(())
    }
}
