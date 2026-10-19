//! "Pair a phone": the QR code and the typed code for the Caw phone app, a
//! two-minute countdown with "New code", the paired phones (which one is
//! connected, each with Remove) and "Let phones type on this computer". The
//! same pairing as `caw-desktop pair`, in the same config folder, so phones
//! paired before stay paired. Nothing opens in a browser.

use super::Ui;
use crate::headless::Command;
use caw_desktop::proto::PairUri;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

pub struct PairWindow {
    window: gtk::Window,
    banner: gtk::Label,
    code_box: gtk::Box,
    qr: gtk::Picture,
    code: gtk::Label,
    address: gtk::Label,
    countdown: gtk::Label,
    outcome_box: gtk::Box,
    outcome: gtk::Label,
    phones: gtk::ListBox,
    phones_switch: gtk::Switch,
    phones_note: gtk::Label,
    shown_uri: RefCell<String>,
    shown_phones: RefCell<String>,
}

pub fn open(ui: &Rc<Ui>) {
    let existing = ui.pair.borrow().clone();
    let win = match existing {
        Some(w) => w,
        None => {
            let w = build(ui);
            *ui.pair.borrow_mut() = Some(w.clone());
            w
        }
    };
    win.opened(ui);
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

fn build(ui: &Rc<Ui>) -> Rc<PairWindow> {
    let window = gtk::Window::builder().application(&ui.app).title("Pair a Phone").default_width(440).default_height(660).hide_on_close(true).build();
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(14).margin_top(24).margin_bottom(24).margin_start(24).margin_end(24).build();

    let banner = gtk::Label::builder().wrap(true).xalign(0.0).visible(false).build();
    banner.add_css_class("caw-banner");
    column.append(&banner);

    let code_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let qr = gtk::Picture::builder().can_shrink(false).halign(gtk::Align::Center).build();
    let code = gtk::Label::builder().selectable(true).build();
    code.add_css_class("caw-code");
    let scan = gtk::Label::new(Some("Scan this with the Caw app on your phone"));
    scan.add_css_class("heading");
    let or_type = dim("Or type the code in Caw: Send to computer → Pair.");
    or_type.set_xalign(0.5);
    let address = dim("");
    address.set_xalign(0.5);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let countdown = dim("");
    countdown.set_hexpand(true);
    let new_code = gtk::Button::with_label("New code");
    row.append(&countdown);
    row.append(&new_code);
    for w in [qr.upcast_ref::<gtk::Widget>(), code.upcast_ref(), scan.upcast_ref(), or_type.upcast_ref(), address.upcast_ref(), row.upcast_ref()] {
        code_box.append(w);
    }
    column.append(&code_box);

    let outcome_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).visible(false).build();
    let outcome = gtk::Label::builder().wrap(true).build();
    outcome.add_css_class("heading");
    let again = gtk::Button::builder().label("New code").halign(gtk::Align::Center).build();
    outcome_box.append(&outcome);
    outcome_box.append(&again);
    column.append(&outcome_box);

    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    column.append(&heading("Paired phones"));
    let phones = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    phones.add_css_class("boxed-list");
    column.append(&phones);
    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let switch_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let switch_label = gtk::Label::builder().label("Let phones type on this computer").xalign(0.0).hexpand(true).build();
    let phones_switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    switch_row.append(&switch_label);
    switch_row.append(&phones_switch);
    column.append(&switch_row);
    let phones_note = dim("While this is off, paired phones can type only while this window is open.");
    column.append(&phones_note);

    let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&column).build();
    window.set_child(Some(&scroll));

    let me = Rc::new(PairWindow {
        window,
        banner,
        code_box,
        qr,
        code,
        address,
        countdown,
        outcome_box,
        outcome,
        phones,
        phones_switch,
        phones_note,
        shown_uri: RefCell::new(String::new()),
        shown_phones: RefCell::new(String::new()),
    });

    for button in [new_code, again] {
        let weak = Rc::downgrade(ui);
        button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.receiver_command(Command::Pair);
            }
        });
    }
    let weak = Rc::downgrade(ui);
    me.phones_switch.connect_active_notify(move |s| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.prefs.borrow().phones_type != s.is_active() {
            ui.set_phones(s.is_active());
        }
    });
    let weak = Rc::downgrade(ui);
    me.window.connect_hide(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        if let Some(state) = ui.receiver.borrow().as_ref() {
            state.set_pair_window(false);
        }
        // No code stays open without the window that shows it.
        if ui.pairing.borrow().is_some() {
            ui.receiver_command(Command::CancelPair);
        }
    });
    // The countdown.
    let weak_ui = Rc::downgrade(ui);
    let weak = Rc::downgrade(&me);
    glib::timeout_add_seconds_local(1, move || {
        let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) else { return glib::ControlFlow::Break };
        if me.window.is_visible() {
            me.refresh(&ui);
        }
        glib::ControlFlow::Continue
    });
    me
}

impl PairWindow {
    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    /// The window opened (or the receiver became ready while it was open): listen, and get a code.
    pub fn opened(&self, ui: &Rc<Ui>) {
        if let Some(state) = ui.receiver.borrow().as_ref() {
            state.set_pair_window(true);
        }
        let fresh = ui.pairing.borrow().as_ref().is_some_and(|p| p.expires > Instant::now());
        if !fresh {
            *ui.pair_outcome.borrow_mut() = None;
            ui.receiver_command(Command::Pair);
        }
    }

    pub fn refresh(&self, ui: &Rc<Ui>) {
        let problem = ui.receiver_problem.borrow().clone().filter(|_| ui.receiver.borrow().is_none());
        self.banner.set_visible(problem.is_some());
        self.banner.set_text(problem.as_deref().unwrap_or(""));

        let pairing = ui.pairing.borrow().clone();
        match pairing {
            Some(p) => {
                self.code_box.set_visible(true);
                self.outcome_box.set_visible(false);
                if *self.shown_uri.borrow() != p.uri {
                    self.qr.set_paintable(qr_texture(&p.uri).as_ref());
                    self.code.set_text(&p.display);
                    let address = PairUri::parse(&p.uri).map(|u| format!("This computer: {}:{}", u.host, u.port)).unwrap_or_default();
                    self.address.set_text(&address);
                    *self.shown_uri.borrow_mut() = p.uri.clone();
                }
                let left = p.expires.saturating_duration_since(Instant::now()).as_secs();
                self.countdown.set_text(&if left > 0 { format!("This code works for {}:{:02}", left / 60, left % 60) } else { "This code has expired.".into() });
            }
            None => {
                self.code_box.set_visible(false);
                self.outcome_box.set_visible(ui.receiver.borrow().is_some());
                self.outcome.set_text(ui.pair_outcome.borrow().as_deref().unwrap_or("Getting a code…"));
                self.shown_uri.borrow_mut().clear();
            }
        }

        // Paired phones: rebuilt only when something about them changed.
        let paired = ui.receiver.borrow().as_ref().map(|s| s.paired_phones()).unwrap_or_default();
        let connected = ui.connected.borrow().clone();
        let key = format!("{paired:?}{connected:?}");
        if *self.shown_phones.borrow() != key {
            *self.shown_phones.borrow_mut() = key;
            while let Some(row) = self.phones.first_child() {
                self.phones.remove(&row);
            }
            if paired.is_empty() {
                self.phones.append(&dim("No phones paired yet."));
            }
            for phone in paired {
                let on = connected.iter().any(|(id, _)| *id == phone.id);
                let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(10).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
                let dot = gtk::Label::new(Some("●"));
                dot.add_css_class(if on { "caw-dot-on" } else { "caw-dot-off" });
                let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
                text.append(&gtk::Label::builder().label(&phone.name).xalign(0.0).build());
                text.append(&dim(if on { "Connected now" } else { "Not connected" }));
                let remove = gtk::Button::with_label("Remove");
                remove.set_valign(gtk::Align::Center);
                let weak = Rc::downgrade(ui);
                let window = self.window.clone();
                let (id, name) = (phone.id.clone(), phone.name.clone());
                remove.connect_clicked(move |_| {
                    let weak = weak.clone();
                    let id = id.clone();
                    let dialog = gtk::AlertDialog::builder()
                        .modal(true)
                        .message(format!("Remove {name}?"))
                        .detail(format!("{name} will stop typing on this computer until you pair it again."))
                        .buttons(["Cancel", "Remove"])
                        .cancel_button(0)
                        .default_button(0)
                        .build();
                    dialog.choose(Some(&window), gtk::gio::Cancellable::NONE, move |answer| {
                        if matches!(answer, Ok(1)) {
                            if let Some(ui) = weak.upgrade() {
                                ui.receiver_command(Command::Remove { id: id.clone() });
                            }
                        }
                    });
                });
                row.append(&dot);
                row.append(&text);
                row.append(&remove);
                self.phones.append(&row);
            }
        }

        let on = ui.prefs.borrow().phones_type;
        if self.phones_switch.is_active() != on {
            self.phones_switch.set_active(on);
        }
        self.phones_note.set_visible(!on);
    }
}

/// The QR code as a texture: black on white, 6 pixels a module, with the quiet zone.
fn qr_texture(text: &str) -> Option<gdk::Texture> {
    let qr = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = qr.width();
    let colors = qr.to_colors();
    let (scale, quiet) = (6usize, 4usize);
    let side = (modules + quiet * 2) * scale;
    let mut pixels = vec![255u8; side * side * 3];
    for (i, c) in colors.iter().enumerate() {
        if *c != qrcode::Color::Dark {
            continue;
        }
        let (mx, my) = (i % modules + quiet, i / modules + quiet);
        for y in my * scale..(my + 1) * scale {
            let start = (y * side + mx * scale) * 3;
            pixels[start..start + scale * 3].fill(0);
        }
    }
    let bytes = glib::Bytes::from_owned(pixels);
    Some(gdk::MemoryTexture::new(side as i32, side as i32, gdk::MemoryFormat::R8g8b8, &bytes, side * 3).upcast())
}
