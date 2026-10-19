//! The app's own switches, in ~/.config/caw-desktop/app.json.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Prefs {
    /// "Dictation": off, the talk key and Crumb do nothing.
    pub dictation: bool,
    /// "Let phones type on this computer": off, the receiver listens only while pairing.
    pub phones_type: bool,
    /// "Show Crumb": the floating Crumb you can click to talk.
    pub show_crumb: bool,
    /// Where Crumb was dragged to: his centre as a fraction of the screen's
    /// width and height (as Caw for Mac stores it). None: bottom centre.
    pub crumb_at: Option<(f64, f64)>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { dictation: true, phones_type: true, show_crumb: true, crumb_at: None }
    }
}

impl Prefs {
    pub fn load(config_dir: &Path) -> Self {
        std::fs::read(config_dir.join("app.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) {
        let result = serde_json::to_vec_pretty(self).map_err(anyhow::Error::from).and_then(|b| crate::fsutil::write_private(&config_dir.join("app.json"), &b));
        if let Err(e) = result {
            super::log(format!("saving app.json: {e:#}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_default_on() {
        let p: Prefs = serde_json::from_str(r#"{"dictation": false}"#).unwrap();
        assert!(!p.dictation && p.phones_type && p.show_crumb && p.crumb_at.is_none());
        let p: Prefs = serde_json::from_str(r#"{"crumb_at": [0.25, 0.9]}"#).unwrap();
        assert_eq!(p.crumb_at, Some((0.25, 0.9)));
    }
}
