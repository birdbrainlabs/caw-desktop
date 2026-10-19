//! "Word fixes": the last dictation (select a word or phrase in it to start a
//! fix for it), your own fixes (word-fixes.personal.json) with add, edit and
//! delete, and below them the list your phone manages (word-fixes.json).
//! Dictation reads both files afresh for every hold, so changes apply to the
//! next dictation at once.

use super::personal::{self, Doc, Entry, CAPITALIZATION};
use super::Ui;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct FixesWindow {
    window: gtk::Window,
    last: gtk::TextView,
    last_hint: gtk::Label,
    form_title: gtk::Label,
    heard: gtk::Entry,
    typed: gtk::Entry,
    capitalization: gtk::DropDown,
    note: gtk::Entry,
    save: gtk::Button,
    cancel: gtk::Button,
    error: gtk::Label,
    list: gtk::ListBox,
    phone_list: gtk::ListBox,
    /// Editing entry i, which was this when the edit began.
    editing: RefCell<Option<(usize, Entry)>>,
    /// Saving is refused while the file can't be read, so it is never overwritten.
    readable: Cell<bool>,
}

pub fn open(ui: &Rc<Ui>) {
    let existing = ui.fixes.borrow().clone();
    let win = match existing {
        Some(w) => w,
        None => {
            let w = build(ui);
            *ui.fixes.borrow_mut() = Some(w.clone());
            w
        }
    };
    win.show_last(ui);
    win.reload(ui);
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

fn build(ui: &Rc<Ui>) -> Rc<FixesWindow> {
    let window = gtk::Window::builder().application(&ui.app).title("Word Fixes").default_width(540).default_height(720).hide_on_close(true).build();
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(20).margin_bottom(20).margin_start(20).margin_end(20).build();

    column.append(&heading("Last dictation"));
    let last = gtk::TextView::builder().editable(false).cursor_visible(false).wrap_mode(gtk::WrapMode::WordChar).top_margin(8).bottom_margin(8).left_margin(8).right_margin(8).build();
    let frame = gtk::Frame::builder().child(&last).build();
    column.append(&frame);
    let last_hint = dim("Select a word or phrase above to fix how Caw hears it.");
    column.append(&last_hint);

    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let form_title = heading("Add a fix");
    column.append(&form_title);
    let grid = gtk::Grid::builder().row_spacing(8).column_spacing(10).build();
    let heard = gtk::Entry::builder().hexpand(true).placeholder_text("what Caw hears, e.g. akme|ackme").build();
    let typed = gtk::Entry::builder().hexpand(true).placeholder_text("what to type instead").build();
    let names: Vec<&str> = CAPITALIZATION.iter().map(|(_, n)| *n).collect();
    let capitalization = gtk::DropDown::from_strings(&names);
    let note = gtk::Entry::builder().hexpand(true).placeholder_text("optional").build();
    for (row, (label, widget)) in [("Heard", heard.upcast_ref::<gtk::Widget>()), ("Type", typed.upcast_ref()), ("Capital letter", capitalization.upcast_ref()), ("Note", note.upcast_ref())].into_iter().enumerate() {
        grid.attach(&gtk::Label::builder().label(label).xalign(1.0).build(), 0, row as i32, 1, 1);
        grid.attach(widget, 1, row as i32, 1, 1);
    }
    column.append(&grid);
    let buttons = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).halign(gtk::Align::End).build();
    let cancel = gtk::Button::builder().label("Cancel").visible(false).build();
    let save = gtk::Button::with_label("Add fix");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    column.append(&buttons);
    let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    error.add_css_class("error");
    column.append(&error);

    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    column.append(&heading("Your word fixes"));
    column.append(&dim("Used first, in this order. Shared format with Caw for Mac and Android."));
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    column.append(&list);

    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    column.append(&heading("From your phone"));
    column.append(&dim("Managed in the Caw app on your phone; used after your own fixes."));
    let phone_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    phone_list.add_css_class("boxed-list");
    column.append(&phone_list);

    let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&column).build();
    window.set_child(Some(&scroll));

    let me = Rc::new(FixesWindow {
        window,
        last,
        last_hint,
        form_title,
        heard,
        typed,
        capitalization,
        note,
        save,
        cancel,
        error,
        list,
        phone_list,
        editing: RefCell::new(None),
        readable: Cell::new(true),
    });

    // Selecting in the last dictation fills in "Heard" for a new fix.
    let weak = Rc::downgrade(&me);
    me.last.buffer().connect_mark_set(move |buffer, _, mark| {
        let Some(me) = weak.upgrade() else { return };
        if mark.name().as_deref() != Some("selection_bound") && mark.name().as_deref() != Some("insert") {
            return;
        }
        let Some((start, end)) = buffer.selection_bounds() else { return };
        let heard = personal::heard_from_selection(&buffer.text(&start, &end, false));
        if heard.is_empty() {
            return;
        }
        if me.editing.borrow().is_some() {
            me.clear_form();
        }
        me.heard.set_text(&heard);
    });
    let weak_ui = Rc::downgrade(ui);
    let weak = Rc::downgrade(&me);
    me.save.connect_clicked(move |_| {
        if let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) {
            me.submit(&ui);
        }
    });
    let weak = Rc::downgrade(&me);
    me.cancel.connect_clicked(move |_| {
        if let Some(me) = weak.upgrade() {
            me.clear_form();
        }
    });
    for entry in [&me.heard, &me.typed, &me.note] {
        let weak_ui = Rc::downgrade(ui);
        let weak = Rc::downgrade(&me);
        entry.connect_activate(move |_| {
            if let (Some(ui), Some(me)) = (weak_ui.upgrade(), weak.upgrade()) {
                me.submit(&ui);
            }
        });
    }
    me
}

impl FixesWindow {
    pub fn show_last(&self, ui: &Ui) {
        let text = ui.last_text.borrow().clone();
        self.last.buffer().set_text(text.as_deref().unwrap_or(""));
        self.last_hint.set_text(if text.is_some() { "Select a word or phrase above to fix how Caw hears it." } else { "Nothing dictated yet. After you dictate, the text shows here (it's never saved)." });
    }

    fn clear_form(&self) {
        *self.editing.borrow_mut() = None;
        for e in [&self.heard, &self.typed, &self.note] {
            e.set_text("");
        }
        self.capitalization.set_selected(0);
        self.form_title.set_text("Add a fix");
        self.save.set_label("Add fix");
        self.cancel.set_visible(false);
        self.error.set_visible(false);
    }

    fn show_error(&self, text: &str) {
        self.error.set_text(text);
        self.error.set_visible(true);
    }

    fn form(&self) -> Entry {
        Entry {
            from: self.heard.text().trim().to_string(),
            to: self.typed.text().to_string(),
            capitalization: CAPITALIZATION.get(self.capitalization.selected() as usize).map(|(k, _)| *k).unwrap_or("matchHeard").to_string(),
            note: self.note.text().trim().to_string(),
        }
    }

    fn submit(self: &Rc<Self>, ui: &Rc<Ui>) {
        if !self.readable.get() {
            return;
        }
        let entry = self.form();
        let editing = self.editing.borrow().clone();
        let result = Doc::load(&ui.config_dir).and_then(|mut doc| {
            match &editing {
                Some((i, was)) => doc.update(*i, was, &entry)?,
                None => doc.add(&entry)?,
            }
            doc.save(&ui.config_dir)
        });
        match result {
            Ok(()) => {
                self.clear_form();
                self.reload(ui);
            }
            Err(e) => {
                self.show_error(&format!("Not saved: {e:#}."));
                if e.to_string().contains("changed meanwhile") {
                    *self.editing.borrow_mut() = None;
                    self.reload(ui);
                }
            }
        }
    }

    fn edit(&self, i: usize, entry: &Entry) {
        *self.editing.borrow_mut() = Some((i, entry.clone()));
        self.heard.set_text(&entry.from);
        self.typed.set_text(&entry.to);
        self.note.set_text(&entry.note);
        self.capitalization.set_selected(CAPITALIZATION.iter().position(|(k, _)| *k == entry.capitalization).unwrap_or(0) as u32);
        self.form_title.set_text("Edit fix");
        self.save.set_label("Save");
        self.cancel.set_visible(true);
        self.error.set_visible(false);
        self.heard.grab_focus();
    }

    fn delete(self: &Rc<Self>, ui: &Rc<Ui>, i: usize, entry: &Entry) {
        let result = Doc::load(&ui.config_dir).and_then(|mut doc| {
            doc.remove(i, entry)?;
            doc.save(&ui.config_dir)
        });
        if let Err(e) = result {
            self.show_error(&format!("Not deleted: {e:#}."));
        }
        if self.editing.borrow().as_ref().is_some_and(|(j, _)| *j == i) {
            self.clear_form();
        }
        self.reload(ui);
    }

    /// Reads both files again and rebuilds the lists.
    pub fn reload(self: &Rc<Self>, ui: &Rc<Ui>) {
        for list in [&self.list, &self.phone_list] {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
        }
        match Doc::load(&ui.config_dir) {
            Ok(doc) => {
                self.readable.set(true);
                self.save.set_sensitive(true);
                let entries = doc.entries();
                if entries.is_empty() {
                    self.list.append(&dim("No fixes yet."));
                }
                for (i, entry) in entries.into_iter().enumerate() {
                    self.list.append(&self.row(ui, i, entry));
                }
            }
            Err(e) => {
                self.readable.set(false);
                self.save.set_sensitive(false);
                self.list.append(&dim(&format!("Your word fixes can't be read, so they can't be changed here until the file is fixed: {e:#}")));
            }
        }
        let phone = crate::fixes::load(&ui.config_dir.join("word-fixes.json"));
        if phone.is_empty() {
            self.phone_list.append(&dim("None."));
        }
        for (heard, fixed) in phone {
            let l = gtk::Label::builder().label(format!("{heard}  →  {fixed}")).xalign(0.0).wrap(true).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
            self.phone_list.append(&l);
        }
    }

    fn row(self: &Rc<Self>, ui: &Rc<Ui>, i: usize, entry: Entry) -> gtk::Box {
        let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        text.append(&gtk::Label::builder().label(format!("{}  →  {}", entry.from, entry.to)).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).build());
        let caps = CAPITALIZATION.iter().find(|(k, _)| *k == entry.capitalization).map(|(_, n)| *n).unwrap_or("");
        let detail = if entry.note.is_empty() { caps.to_string() } else { format!("{caps} · {}", entry.note) };
        text.append(&dim(&detail));
        let edit = gtk::Button::with_label("Edit");
        let delete = gtk::Button::with_label("Delete");
        for b in [&edit, &delete] {
            b.set_valign(gtk::Align::Center);
        }
        let weak = Rc::downgrade(self);
        let e = entry.clone();
        edit.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.edit(i, &e);
            }
        });
        let weak = Rc::downgrade(self);
        let weak_ui = Rc::downgrade(ui);
        delete.connect_clicked(move |_| {
            if let (Some(me), Some(ui)) = (weak.upgrade(), weak_ui.upgrade()) {
                me.delete(&ui, i, &entry);
            }
        });
        row.append(&text);
        row.append(&edit);
        row.append(&delete);
        row
    }
}
