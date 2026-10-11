//! Desktop settings the phone can read and change (protocol: settings_get /
//! settings_set). Stored in ~/.config/caw-desktop/settings.json; word fixes
//! stay in their own file (see fixes.rs).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SPEED_RANGE: (f64, f64) = (0.1, 10.0);
pub const ACCEL_RANGE: (f64, f64) = (0.0, 1.0);
pub const MAX_FIXES: usize = 500;
pub const MAX_FIX_LEN: usize = 100;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub pointer_speed: f64,
    pub pointer_accel: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { pointer_speed: 1.5, pointer_accel: 0.05 }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

pub fn check_range(name: &str, value: f64, (lo, hi): (f64, f64)) -> Result<()> {
    if !(lo..=hi).contains(&value) {
        bail!("{name} must be between {lo} and {hi}");
    }
    Ok(())
}

pub fn check_fixes(fixes: &crate::fixes::Fixes) -> Result<()> {
    if fixes.len() > MAX_FIXES {
        bail!("at most {MAX_FIXES} word fixes");
    }
    for (heard, fixed) in fixes {
        if heard.trim().is_empty() || heard.len() > MAX_FIX_LEN || fixed.len() > MAX_FIX_LEN {
            bail!("each word fix needs a phrase of 1–{MAX_FIX_LEN} characters");
        }
    }
    Ok(())
}
