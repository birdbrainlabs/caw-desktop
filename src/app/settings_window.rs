//! "Settings": the hold-to-talk key (Caw never reads the keyboard: on
//! Hyprland the desktop's shortcut runs `talk start` and `talk stop`, so this
//! shows the key bound now and the exact lines to bind one; on GNOME and KDE
//! the desktop's shortcuts portal holds the key, and choosing another one here
//! asks the desktop again), and the switches.

use super::talk_key::Status;
use super::Ui;
use crate::dictate::{binding, Settings};
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub struct SettingsWindow {
    window: gtk::Window,
    bound: gtk::Label,
    key: gtk::DropDown,
    instructions: gtk::Label,
    lines: gtk::TextView,
    lines_frame: gtk::Frame,
    copy: gtk::Button,
    dictation: gtk::Switch,
    phones: gtk::Switch,
    crumb: gtk::Switch,
    crumb_note: gtk::Label,
    updating: Cell<bool>,
}

pub fn open(ui: &Rc<Ui>) {
    let existing = ui.settings.borrow().clone();
    let win = match existing {
        Some(w) => w,
        None => {
            let w = build(ui);
            *ui.settings.borrow_mut() = Some(w.clone());
            w
        }
    };
    ui.refresh_talk_hint();
    win.show_key(ui, None);
    win.refresh(ui);
    win.window.present();
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).build();
    l.add_css_class("heading");
    l
}

fn dim(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class("dim-label");
    l
}

fn switch_row(column: &gtk::Box, title: &str) -> gtk::Switch {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Label::builder().label(title).xalign(0.0).hexpand(true).build());
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    row.append(&switch);
    column.append(&row);
    switch
}

fn build(ui: &Rc<Ui>) -> Rc<SettingsWindow> {
    let window = gtk::Window::builder().application(&ui.app).title("Caw Settings").default_width(560).default_height(600).hide_on_close(true).build();
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(20).margin_bottom(20).margin_start(20).margin_end(20).build();

    // GNOME: pressed to start and again to stop, not held.
    column.append(&heading(if super::talk_key::press_to_toggle() { "Talk key" } else { "Hold-to-talk key" }));
    let bound = gtk::Label::builder().xalign(0.0).wrap(true).build();
    column.append(&bound);
    let key_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    key_row.append(&gtk::Label::new(Some("Key")));
    let names: Vec<&str> = binding::CHOICES.iter().map(|(_, n)| *n).collect();
    let key = gtk::DropDown::from_strings(&names);
    key_row.append(&key);
    column.append(&key_row);
    let instructions = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).build();
    column.append(&instructions);
    let lines = gtk::TextView::builder().editable(false).monospace(true).wrap_mode(gtk::WrapMode::Char).top_margin(8).bottom_margin(8).left_margin(8).right_margin(8).build();
    let lines_frame = gtk::Frame::builder().child(&lines).build();
    column.append(&lines_frame);
    let copy = gtk::Button::builder().label("Copy lines").halign(gtk::Align::End).build();
    column.append(&copy);

    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let dictation = switch_row(&column, "Dictation");
    column.append(&dim("Off, the talk key and Crumb do nothing."));
    let crumb = switch_row(&column, "Show Crumb");
    let crumb_note = dim("Crumb floats above your windows. Click him to start or stop talking; drag him anywhere.");
    column.append(&crumb_note);
    let phones = switch_row(&column, "Let phones type on this computer");

    let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&column).build();
    window.set_child(Some(&scroll));

    let me = Rc::new(SettingsWindow { window, bound, key, instructions, lines, lines_frame, copy, dictation, phones, crumb, crumb_note, updating: Cell::new(false) });

    let weak_ui = Rc::downgrade(ui);
    let weak = Rc::downgrade(&me);
    me.key.connect_selected_notify(move |dd| {
        let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) else { return };
        if me.updating.get() {
            return;
        }
        let Some((talk_key, _)) = binding::CHOICES.get(dd.selected() as usize) else { return };
        let mut settings = Settings::load(&ui.config_dir);
        settings.talk_key = Some(talk_key.to_string());
        if let Err(e) = settings.save(&ui.config_dir) {
            super::log(format!("saving dictation.json: {e:#}"));
        }
        ui.rebind_talk_key(talk_key);
        me.show_key(&ui, Some(talk_key));
    });
    let weak = Rc::downgrade(&me);
    me.copy.connect_clicked(move |b| {
        let Some(me) = weak.upgrade() else { return };
        let buffer = me.lines.buffer();
        b.clipboard().set_text(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false));
    });
    let weak_ui = Rc::downgrade(ui);
    let weak = Rc::downgrade(&me);
    me.dictation.connect_active_notify(move |s| {
        let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) else { return };
        if !me.updating.get() && ui.prefs.borrow().dictation != s.is_active() {
            ui.set_dictation(s.is_active());
        }
    });
    let weak_ui = Rc::downgrade(ui);
    let weak = Rc::downgrade(&me);
    me.crumb.connect_active_notify(move |s| {
        let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) else { return };
        if !me.updating.get() && ui.prefs.borrow().show_crumb != s.is_active() {
            ui.set_show_crumb(s.is_active());
        }
    });
    let weak_ui = Rc::downgrade(ui);
    let weak = Rc::downgrade(&me);
    me.phones.connect_active_notify(move |s| {
        let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) else { return };
        if !me.updating.get() && ui.prefs.borrow().phones_type != s.is_active() {
            ui.set_phones(s.is_active());
        }
    });
    me
}

impl SettingsWindow {
    /// The bound key, and the lines for `chosen` (else the key bound now, else the setting).
    fn show_key(&self, ui: &Ui, chosen: Option<&str>) {
        let exe = binding::exe();
        self.show_lines(true);
        if !binding::on_hyprland() {
            let key = chosen.map(String::from).unwrap_or_else(|| Settings::load(&ui.config_dir).talk_key());
            self.select(&key);
            let status = ui.talk_status.borrow().clone();
            match status {
                Some(Status::Bound(name)) if chosen.is_none() && super::talk_key::press_to_toggle() => {
                    self.bound.set_text(&format!("Press {} to start talking, and press it again to stop. Your desktop looks after the key: choose another one here and it asks you again.", ui.talk_key_name(&name)));
                    self.show_lines(false);
                    return;
                }
                Some(Status::Bound(name)) if chosen.is_none() => {
                    self.bound.set_text(&format!("Hold {} to talk. Your desktop looks after the key: choose another one here and it asks you again.", ui.talk_key_name(&name)));
                    self.show_lines(false);
                    return;
                }
                Some(Status::Asking) | Some(Status::Bound(_)) => {
                    self.bound.set_text(&format!("Your desktop is asking whether Caw may use {}. Answer its window.", binding::describe(&key)));
                    self.show_lines(false);
                    return;
                }
                // GNOME without a key from Caw: one shortcut of the person's own, press to start, press to stop.
                _ if super::talk_key::on_gnome() => {
                    self.bound.set_text("To talk with a key, add one keyboard shortcut in Ubuntu's Settings.");
                    self.instructions.set_text("Open Settings, then Keyboard, then View and Customize Shortcuts, then Custom Shortcuts. Add a shortcut called Caw, paste the line below as its command, and press the key you want to use. Press that key once to start talking and again to stop.");
                    self.lines.buffer().set_text(&format!("{exe} talk toggle"));
                    return;
                }
                Some(Status::Declined) => self.bound.set_text("Your desktop didn't give Caw a talk key. Choose one here to be asked again, or make a shortcut yourself:"),
                Some(Status::Unavailable(why)) => self.bound.set_text(&format!("Caw can't ask this desktop for a talk key ({why}). Your desktop's keyboard shortcut can start and stop talking instead.")),
                None => self.bound.set_text("Caw doesn't read the keyboard itself: your desktop's keyboard shortcut starts and stops talking."),
            }
            let crumb = if super::crumb::supported() { " You can also click Crumb." } else { "" };
            self.instructions.set_text(&format!("Make a shortcut for {} that runs the first command when the key is pressed and the second when it is let go.{crumb}", binding::describe(&key)));
            self.lines.buffer().set_text(&format!("{exe} talk start\n{exe} talk stop"));
            return;
        }
        let dir = binding::hypr_dir();
        let style = binding::style(&dir);
        let found = binding::find(&dir);
        match &found {
            Some(b) => {
                let key = binding::talk_key_for(&b.keys).map(binding::describe).unwrap_or_else(|| b.keys.clone());
                self.bound.set_text(&format!("Bound now: {key} (in {})", b.file.display()));
            }
            None => self.bound.set_text("No key is bound to Caw in your Hyprland config yet."),
        }
        let key = chosen
            .map(String::from)
            .or_else(|| found.as_ref().and_then(|b| binding::talk_key_for(&b.keys)).map(String::from))
            .unwrap_or_else(|| Settings::load(&ui.config_dir).talk_key());
        let file = binding::target_file(&dir, style);
        self.instructions.set_text(&format!(
            "To hold {} to talk, put these lines in {}{}. Hyprland reloads it when the file is saved.",
            binding::describe(&key),
            file.display(),
            if found.is_some() { ", in place of the Caw lines there now" } else { "" }
        ));
        self.lines.buffer().set_text(&binding::lines(style, &key, &exe).join("\n"));
        self.select(&key);
    }

    fn show_lines(&self, on: bool) {
        self.instructions.set_visible(on);
        self.lines_frame.set_visible(on);
        self.copy.set_visible(on);
    }

    fn select(&self, talk_key: &str) {
        if let Some(i) = binding::CHOICES.iter().position(|(k, _)| k.eq_ignore_ascii_case(talk_key)) {
            self.updating.set(true);
            self.key.set_selected(i as u32);
            self.updating.set(false);
        }
    }

    pub fn refresh(&self, ui: &Ui) {
        if !binding::on_hyprland() {
            self.show_key(ui, None);
        }
        self.updating.set(true);
        let prefs = ui.prefs.borrow().clone();
        self.dictation.set_active(prefs.dictation);
        self.phones.set_active(prefs.phones_type);
        self.crumb.set_active(prefs.show_crumb);
        self.crumb.set_sensitive(super::crumb::supported());
        if !super::crumb::supported() {
            self.crumb_note.set_text(if cfg!(feature = "app") {
                "This desktop can't float Crumb above other windows (it has no layer-shell)."
            } else {
                "This version of Caw can't float Crumb above other windows."
            });
        }
        self.updating.set(false);
    }
}
