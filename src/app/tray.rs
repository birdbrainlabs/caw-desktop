//! The tray icon (StatusNotifierItem): Caw's waveform in a ring, a solid disc
//! while listening (as Caw for Mac's waveform.circle / waveform.circle.fill),
//! slashed while dictation is off. They are symbolic icons the package
//! installs (packaging/linux/icons), so the panel colours them to match;
//! without them (a build run from its folder) it falls back to the icon
//! theme's microphone. Menus stay flat: some trays can't open submenus.

use super::Command;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub dictation_on: bool,
    pub listening: bool,
    /// "Hold Fn to talk", "Listening…", "Dictation is off", or what went wrong.
    pub dictation_line: String,
    pub phones_on: bool,
    /// "Connected: Pixel" or "No phone connected".
    pub phone_line: String,
    pub phone_connected: bool,
}

pub struct Tray {
    pub view: View,
    pub commands: async_channel::Sender<Command>,
    /// Caw's own icons are installed (in the hicolor theme).
    pub own_icons: bool,
}

/// The installed icon looked for at start.
pub const OWN_ICON: &str = "caw-waveform-symbolic";

impl Tray {
    fn send(&self, command: Command) {
        let _ = self.commands.try_send(command);
    }

    /// One line, for the tooltip: some bars show only its title.
    fn summary(&self) -> String {
        let talk = if !self.view.dictation_on {
            "dictation off"
        } else if self.view.listening {
            "Listening"
        } else {
            "ready"
        };
        let phone = if !self.view.phones_on {
            "phones off"
        } else if self.view.phone_connected {
            "phone connected"
        } else {
            "no phone"
        };
        format!("Caw: {talk} · {phone}")
    }
}

impl ksni::Tray for Tray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "caw-desktop".into()
    }

    fn title(&self) -> String {
        self.summary()
    }

    fn icon_name(&self) -> String {
        if self.own_icons {
            return if !self.view.dictation_on {
                "caw-waveform-off-symbolic"
            } else if self.view.listening {
                "caw-waveform-listening-symbolic"
            } else {
                OWN_ICON
            }
            .into();
        }
        if !self.view.dictation_on {
            "microphone-disabled-symbolic"
        } else if self.view.listening {
            "microphone-sensitivity-high-symbolic"
        } else {
            "audio-input-microphone-symbolic"
        }
        .into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.summary(), description: format!("{}\n{}", self.view.dictation_line, self.view.phone_line), ..Default::default() }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, StandardItem};
        use ksni::MenuItem;
        let info = |label: &str| -> MenuItem<Self> { StandardItem { label: label.replace('_', "__"), enabled: false, ..Default::default() }.into() };
        let action = |label: &str, command: Command| -> MenuItem<Self> {
            StandardItem { label: label.into(), activate: Box::new(move |t: &mut Self| t.send(command.clone())), ..Default::default() }.into()
        };
        vec![
            info(&self.view.dictation_line),
            info(&self.view.phone_line),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Dictation".into(),
                checked: self.view.dictation_on,
                activate: Box::new(|t: &mut Self| t.send(Command::SetDictation(!t.view.dictation_on))),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Let phones type on this computer".into(),
                checked: self.view.phones_on,
                activate: Box::new(|t: &mut Self| t.send(Command::SetPhones(!t.view.phones_on))),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            action("Word fixes…", Command::OpenFixes),
            action("Pair a phone…", Command::OpenPair),
            action("Settings…", Command::OpenSettings),
            MenuItem::Separator,
            action("Quit", Command::Quit),
        ]
    }
}
