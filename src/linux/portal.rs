//! GNOME and KDE Plasma on Wayland: the remote-desktop portal, the same one
//! screen-sharing and remote-support apps use. The desktop asks once whether
//! Caw Desktop may control the keyboard and mouse; a restore token saved in
//! the config folder lets it skip the question next time.

use anyhow::{Context, Result};
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::{PersistMode, Session};
use std::path::Path;
use std::time::Duration;

const KEY_GAP: Duration = Duration::from_millis(3);

pub struct Portal {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

impl Portal {
    pub async fn open(token_file: &Path) -> Result<Self> {
        let proxy = RemoteDesktop::new().await.context("no remote-desktop portal")?;
        let session = proxy.create_session(Default::default()).await?;
        let token = std::fs::read_to_string(token_file).ok();
        let devices = SelectDevicesOptions::default()
            .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(token.as_deref().map(str::trim));
        proxy.select_devices(&session, devices).await?.response()?;
        let started = proxy
            .start(&session, None, Default::default())
            .await?
            .response()
            .context("the desktop did not allow keyboard and mouse control")?;
        // Tokens are single-use: each start hands out the one for next time.
        if let Some(token) = started.restore_token() {
            let _ = std::fs::write(token_file, token);
        }
        Ok(Self { proxy, session })
    }

    pub async fn tap_keysyms(&self, keysyms: &[u32]) -> Result<()> {
        for &sym in keysyms {
            for state in [KeyState::Pressed, KeyState::Released] {
                self.proxy.notify_keyboard_keysym(&self.session, sym as i32, state, Default::default()).await?;
            }
            // Some portals hand keys on asynchronously and can swap two that arrive together.
            tokio::time::sleep(KEY_GAP).await;
        }
        Ok(())
    }

    /// Holds the modifiers, taps `key` `count` times, then releases the
    /// modifiers in reverse order, even if a tap failed. All are evdev
    /// keycodes: desktops match shortcuts by key position, and a keysym
    /// such as "T" would bring its own Shift.
    pub async fn tap_combo(&self, mods: &[u32], key: u32, count: u32) -> Result<()> {
        let mut held = Vec::new();
        let mut result = Ok(());
        for &m in mods {
            result = self.keycode(m, KeyState::Pressed).await;
            if result.is_err() {
                break;
            }
            held.push(m);
        }
        for _ in 0..count {
            if result.is_err() {
                break;
            }
            result = match self.keycode(key, KeyState::Pressed).await {
                Ok(()) => self.keycode(key, KeyState::Released).await,
                e => e,
            };
        }
        for &m in held.iter().rev() {
            let released = self.keycode(m, KeyState::Released).await;
            result = result.and(released);
        }
        result
    }

    async fn keycode(&self, code: u32, state: KeyState) -> Result<()> {
        self.proxy.notify_keyboard_keycode(&self.session, code as i32, state, Default::default()).await?;
        tokio::time::sleep(KEY_GAP).await;
        Ok(())
    }

    pub async fn motion(&self, dx: f64, dy: f64) -> Result<()> {
        Ok(self.proxy.notify_pointer_motion(&self.session, dx, dy, Default::default()).await?)
    }

    /// `button` is a Linux evdev code such as BTN_LEFT.
    pub async fn button(&self, button: u32, pressed: bool) -> Result<()> {
        let state = if pressed { KeyState::Pressed } else { KeyState::Released };
        Ok(self.proxy.notify_pointer_button(&self.session, button as i32, state, Default::default()).await?)
    }

    pub async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        Ok(self.proxy.notify_pointer_axis(&self.session, dx, dy, Default::default()).await?)
    }
}
