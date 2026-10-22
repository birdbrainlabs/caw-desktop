//! The floating Crumb (Caw for Mac's floating orb): Crumb's own page in a
//! transparent WebKit view on a layer-shell overlay surface, which never takes
//! the keyboard, so text keeps landing where you were typing. Click him to
//! start or stop talking; drag him anywhere (where he is is remembered).
//!
//! He is told only the state and the microphone level (`window.updateCrumb`,
//! as Caw for Mac and Android do): 30 times a second while something is
//! happening, 4 times a second otherwise. Without WebKitGTK 6.0, or if his
//! page can't run, the Mac's plain orb is drawn instead. Without layer-shell
//! (GNOME) there is no floating Crumb at all.

use super::{log, orb, webkit};
use crate::dictate::Activity;
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use layer::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Crumb's diameter as on the Mac (`orbDiameter` for the mascot), and his window.
const CRUMB_DIAMETER: f64 = 72.0;
/// The space the Mac's view keeps around the drawing on each side.
const PADDING: i32 = 4;
/// The warning pill, as on the Mac.
const MESSAGE_SIZE: (i32, i32) = (380, 58);
/// Caw for Mac's insets: 4 from the bottom, 8 from the right edge.
const BOTTOM_INSET: i32 = 4;
const EDGE_INSET: i32 = 8;
const DRAG_THRESHOLD: f64 = 4.0;
/// How long a warning stays unless clicked away.
const MESSAGE_FOR: Duration = Duration::from_secs(8);

/// His page, shipped whole (assets/crumb, as in Caw for Android).
const PAGE: &[(&str, &[u8])] = &[
    ("index.html", include_bytes!("../../assets/crumb/index.html")),
    ("crumb.bundle.js", include_bytes!("../../assets/crumb/crumb.bundle.js")),
    ("THREE-LICENSE.txt", include_bytes!("../../assets/crumb/THREE-LICENSE.txt")),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Look {
    /// His page is shown (or loading).
    Crumb,
    /// The plain orb.
    Orb,
}

pub struct Hooks {
    pub click: Box<dyn Fn()>,
    /// Dropped somewhere new: his centre as fractions of the screen (x from the left, y from the bottom).
    pub moved: Box<dyn Fn((f64, f64))>,
    /// The microphone level now, 0..1.
    pub level: Box<dyn Fn() -> f32>,
}

pub struct Crumb {
    window: gtk::Window,
    stack: gtk::Stack,
    canvas: gtk::DrawingArea,
    spinner: gtk::DrawingArea,
    message: gtk::Label,
    web: RefCell<Option<webkit::WebView>>,
    web_ready: Cell<bool>,
    crashes: Cell<u32>,
    look: Cell<Look>,
    activity: Cell<Activity>,
    showing_message: Cell<Option<Instant>>,
    delivery: Cell<u32>,
    level: Cell<f64>,
    last_json: RefCell<String>,
    /// Where the orb's window is: (left, bottom) margins, in pixels.
    at: Cell<(i32, i32)>,
    anchor: Cell<Option<(f64, f64)>>,
    /// While a drag is on: whether it has travelled yet.
    drag: Cell<Option<bool>>,
    /// Frames still to paint before the last move counts as shown (see the drag).
    settle: Cell<u8>,
    /// On Hyprland, where the drag began: the pointer on screen, and where he was.
    drag_origin: Cell<Option<((i32, i32), (i32, i32))>>,
    hooks: Hooks,
    started: Instant,
    tick_due: Cell<bool>,
    shown: Cell<bool>,
}

#[cfg(feature = "app")]
mod layer {
    pub use gtk4_layer_shell::{is_supported, Edge, KeyboardMode, Layer, LayerShell};
}

/// Built without gtk4-layer-shell (feature "app-base", the Ubuntu .deb): Crumb never floats.
#[cfg(not(feature = "app"))]
mod layer {
    pub enum Edge {
        Left,
        Bottom,
    }
    pub enum Layer {
        Overlay,
    }
    pub enum KeyboardMode {
        None,
    }
    pub trait LayerShell {
        fn init_layer_shell(&self) {}
        fn set_layer(&self, _: Layer) {}
        fn set_keyboard_mode(&self, _: KeyboardMode) {}
        fn set_namespace(&self, _: Option<&str>) {}
        fn set_anchor(&self, _: Edge, _: bool) {}
        fn set_margin(&self, _: Edge, _: i32) {}
    }
    impl LayerShell for gtk::Window {}
    pub fn is_supported() -> bool {
        false
    }
}

/// Whether this desktop can float Crumb (layer-shell: Hyprland, Sway, KDE; not GNOME).
pub fn supported() -> bool {
    // Layer-shell is Wayland only (and asks GTK's Wayland display, so check that first).
    gdk::Display::default().is_some_and(|d| d.type_().name() == "GdkWaylandDisplay") && layer::is_supported()
}

/// Writes his page to a private folder for WebKit (the runtime folder: memory, gone at logout).
fn page_folder() -> Option<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = dirs::runtime_dir()?.join("caw-desktop-crumb");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).ok()?;
    for (name, bytes) in PAGE {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(*bytes) {
            std::fs::write(&path, bytes).ok()?;
        }
    }
    Some(dir)
}

impl Crumb {
    pub fn new(app: &gtk::Application, anchor: Option<(f64, f64)>, hooks: Hooks) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).title("Caw").decorated(false).resizable(false).build();
        window.add_css_class("caw-crumb");
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_namespace(Some("caw-crumb"));
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Bottom, true);

        let stack = gtk::Stack::builder().hhomogeneous(false).vhomogeneous(false).interpolate_size(false).build();
        let canvas = gtk::DrawingArea::new();
        stack.add_named(&canvas, Some("orb"));
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pill.add_css_class("caw-message");
        let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
        icon.set_pixel_size(12);
        let message = gtk::Label::builder().wrap(true).lines(2).ellipsize(gtk::pango::EllipsizeMode::End).xalign(0.0).hexpand(true).build();
        message.add_css_class("caw-message-text");
        pill.append(&icon);
        pill.append(&message);
        stack.add_named(&pill, Some("message"));

        let spinner = gtk::DrawingArea::new();
        spinner.set_can_target(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&spinner);
        window.set_child(Some(&overlay));

        let me = Rc::new(Self {
            window,
            stack,
            canvas,
            spinner,
            message,
            web: RefCell::new(None),
            web_ready: Cell::new(false),
            crashes: Cell::new(0),
            look: Cell::new(Look::Orb),
            activity: Cell::new(Activity::Idle),
            showing_message: Cell::new(None),
            delivery: Cell::new(0),
            level: Cell::new(0.0),
            last_json: RefCell::new(String::new()),
            at: Cell::new((0, BOTTOM_INSET)),
            anchor: Cell::new(anchor),
            drag: Cell::new(None),
            settle: Cell::new(0),
            drag_origin: Cell::new(None),
            hooks,
            started: Instant::now(),
            tick_due: Cell::new(false),
            shown: Cell::new(false),
        });

        let weak = Rc::downgrade(&me);
        me.canvas.set_draw_func(move |_, cr, w, h| {
            if let Some(me) = weak.upgrade() {
                let t = me.started.elapsed().as_secs_f64() * 2.1;
                let talking = me.activity.get() == Activity::Listening;
                orb::draw(cr, w as f64, h as f64, orb::DIAMETER, talking, if talking { me.level.get() } else { 0.0 }, if talking { t } else { 0.0 });
            }
        });
        let weak = Rc::downgrade(&me);
        me.spinner.set_draw_func(move |_, cr, w, h| {
            if let Some(me) = weak.upgrade() {
                if me.activity.get() == Activity::Working && me.showing_message.get().is_none() {
                    orb::draw_spinner(cr, w as f64, h as f64, 9.0, me.started.elapsed().as_secs_f64());
                }
            }
        });

        // Drag anywhere on him to move him; a press that never travels is a click.
        let gesture = gtk::GestureDrag::new();
        gesture.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(&me);
        gesture.connect_drag_begin(move |_, _, _| {
            if let Some(me) = weak.upgrade() {
                me.drag.set(Some(false));
                me.drag_origin.set(hyprland_cursor().map(|c| (c, me.at.get())));
            }
        });
        let weak = Rc::downgrade(&me);
        gesture.connect_drag_update(move |_, dx, dy| {
            let Some(me) = weak.upgrade() else { return };
            let Some(moved) = me.drag.get() else { return };
            if !moved && dx.hypot(dy) < DRAG_THRESHOLD {
                return;
            }
            me.drag.set(Some(true));
            if me.showing_message.get().is_some() {
                return;
            }
            // Hyprland slides layers to a new place, and pointer offsets are from wherever the
            // slide has got to, so they can't be trusted there; its own pointer position can.
            if let (Some((c0, at0)), Some(c)) = (me.drag_origin.get(), hyprland_cursor()) {
                me.at.set(me.clamp(at0.0 + c.0 - c0.0, at0.1 - (c.1 - c0.1)));
                me.place();
                return;
            }
            // Offsets are from the press, in the window's own coordinates, which move with
            // it: so the new place is where it is now plus the offset, but only once the
            // compositor has the last move. A frame drawn after the one carrying the move
            // means it has; until then an offset might count that move twice.
            if me.settle.get() > 0 || me.showing_message.get().is_some() {
                return;
            }
            let (left, bottom) = me.at.get();
            me.at.set(me.clamp(left + dx.round() as i32, bottom - dy.round() as i32));
            me.place();
            me.settle.set(2);
            if let Some(clock) = me.window.frame_clock() {
                clock.request_phase(gdk::FrameClockPhase::PAINT);
            }
            // Should frames stop coming (a hidden output), don't stay stuck.
            let weak = Rc::downgrade(&me);
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                if let Some(me) = weak.upgrade() {
                    me.settle.set(0);
                }
            });
        });
        let weak = Rc::downgrade(&me);
        gesture.connect_drag_end(move |_, _, _| {
            let Some(me) = weak.upgrade() else { return };
            let moved = me.drag.take().unwrap_or(false);
            // The last bit of travel can arrive with the release instead of as a move.
            if let (true, Some((c0, at0)), Some(c), None) = (moved, me.drag_origin.get(), hyprland_cursor(), me.showing_message.get()) {
                me.at.set(me.clamp(at0.0 + c.0 - c0.0, at0.1 - (c.1 - c0.1)));
                me.place();
            }
            me.drag_origin.set(None);
            if me.showing_message.get().is_some() {
                me.dismiss_message();
            } else if moved {
                let anchor = me.anchor_now();
                me.anchor.set(Some(anchor));
                (me.hooks.moved)(anchor);
            } else {
                (me.hooks.click)();
            }
        });
        overlay.add_controller(gesture);

        let weak = Rc::downgrade(&me);
        me.window.connect_map(move |_| {
            if let Some(me) = weak.upgrade() {
                me.place();
            }
        });
        let weak = Rc::downgrade(&me);
        me.window.connect_realize(move |w| {
            let Some(clock) = w.frame_clock() else { return };
            let weak = weak.clone();
            clock.connect_after_paint(move |clock| {
                let Some(me) = weak.upgrade() else { return };
                let left = me.settle.get().saturating_sub(1);
                me.settle.set(left);
                if left > 0 {
                    clock.request_phase(gdk::FrameClockPhase::PAINT);
                }
            });
        });
        me
    }

    fn side(&self) -> i32 {
        let diameter = if self.look.get() == Look::Crumb { CRUMB_DIAMETER } else { orb::DIAMETER };
        orb::canvas_side(diameter).round() as i32 + PADDING * 2
    }

    fn screen(&self) -> (i32, i32) {
        let display = gdk::Display::default();
        let monitor = display.as_ref().and_then(|d| {
            self.window.surface().and_then(|s| d.monitor_at_surface(&s)).or_else(|| d.monitors().item(0).and_then(|m| m.downcast::<gdk::Monitor>().ok()))
        });
        monitor.map(|m| (m.geometry().width(), m.geometry().height())).unwrap_or((1280, 800))
    }

    fn clamp(&self, left: i32, bottom: i32) -> (i32, i32) {
        let (w, h) = self.screen();
        let side = self.side();
        (left.clamp(0, (w - side).max(0)), bottom.clamp(0, (h - side).max(0)))
    }

    fn anchor_now(&self) -> (f64, f64) {
        let (w, h) = self.screen();
        let half = self.side() as f64 / 2.0;
        let (left, bottom) = self.at.get();
        (((left as f64 + half) / w as f64).clamp(0.0, 1.0), ((bottom as f64 + half) / h as f64).clamp(0.0, 1.0))
    }

    /// Back to the stored place (bottom right if none, as on the Mac), for the current size.
    fn home(&self) {
        let (w, h) = self.screen();
        let side = self.side();
        let at = match self.anchor.get() {
            Some((x, y)) => ((x * w as f64) as i32 - side / 2, (y * h as f64) as i32 - side / 2),
            None => (w - side - EDGE_INSET, BOTTOM_INSET),
        };
        self.at.set(self.clamp(at.0, at.1));
    }

    /// Sizes and places the window for what it shows now. A warning is centred on him.
    fn place(&self) {
        let side = self.side();
        let (left, bottom) = self.at.get();
        let (w, h, left) = if self.showing_message.get().is_some() {
            let (sw, _) = self.screen();
            (MESSAGE_SIZE.0, MESSAGE_SIZE.1, (left + side / 2 - MESSAGE_SIZE.0 / 2).clamp(0, (sw - MESSAGE_SIZE.0).max(0)))
        } else {
            (side, side, left)
        };
        self.window.set_margin(Edge::Left, left);
        self.window.set_margin(Edge::Bottom, bottom);
        self.window.set_default_size(w, h);
        self.stack.set_size_request(w, h);
        self.spinner.set_size_request(w, h);
        // Only his middle takes clicks; the transparent rest of the square lets them through.
        if let Some(surface) = self.window.surface() {
            let r = if self.showing_message.get().is_some() {
                cairo::RectangleInt::new(0, 0, w, h)
            } else {
                let hit = (side as f64 * if self.look.get() == Look::Crumb { 0.62 } else { 0.5 }) as i32;
                cairo::RectangleInt::new((side - hit) / 2, (side - hit) / 2, hit, hit)
            };
            surface.set_input_region(Some(&cairo::Region::create_rectangle(&r)));
        }
    }

    pub fn show(self: &Rc<Self>) {
        if self.shown.replace(true) {
            return;
        }
        if self.web.borrow().is_none() {
            self.load_page();
        }
        self.home();
        self.place();
        self.window.set_visible(true);
        self.schedule_tick();
    }

    /// Hides him and lets WebKit's processes go (they hold most of his memory).
    pub fn hide(&self) {
        if !self.shown.replace(false) {
            return;
        }
        self.window.set_visible(false);
        self.drop_page();
    }

    fn drop_page(&self) {
        if let Some(web) = self.web.take() {
            self.stack.remove(&web.widget);
        }
        self.web_ready.set(false);
        self.last_json.borrow_mut().clear();
        self.set_look(Look::Orb);
    }

    fn set_look(&self, look: Look) {
        self.look.set(look);
        if self.showing_message.get().is_none() {
            self.stack.set_visible_child_name(if look == Look::Crumb { "crumb" } else { "orb" });
        }
        if self.shown.get() {
            self.home();
            self.place();
        }
    }

    fn load_page(self: &Rc<Self>) {
        let Some(folder) = page_folder() else {
            log("Crumb: no private runtime folder for his page; showing the plain orb");
            return self.set_look(Look::Orb);
        };
        let web = match webkit::WebView::new(&folder) {
            Ok(w) => w,
            Err(why) => {
                log(format!("Crumb: {why}; showing the plain orb"));
                return self.set_look(Look::Orb);
            }
        };
        let canvas = orb::canvas_side(CRUMB_DIAMETER).round() as i32;
        web.widget.set_size_request(canvas, canvas);
        web.widget.set_margin_start(PADDING);
        web.widget.set_margin_end(PADDING);
        web.widget.set_margin_top(PADDING);
        web.widget.set_margin_bottom(PADDING);
        // Clicks and drags go to the window, never into the page.
        web.widget.set_can_target(false);
        web.widget.set_can_focus(false);
        self.stack.add_named(&web.widget, Some("crumb"));
        web.connect_loading_changed(|view| {
            if view.is_loading() {
                return;
            }
            // Did his page start? (Its script defines updateCrumb; the title carries the answer back.)
            view.run("document.title = (typeof window.updateCrumb === 'function') ? 'caw-crumb-ready' : 'caw-crumb-failed'");
        });
        let weak = Rc::downgrade(self);
        web.connect_title_changed(move |view| {
            let Some(me) = weak.upgrade() else { return };
            match view.title().as_str() {
                "caw-crumb-ready" => {
                    me.web_ready.set(true);
                    me.last_json.borrow_mut().clear();
                    me.send_state();
                }
                "caw-crumb-failed" => {
                    log("Crumb: his page couldn't start (WebGL?); showing the plain orb");
                    me.drop_page();
                }
                _ => {}
            }
        });
        let weak = Rc::downgrade(self);
        web.connect_crashed(move |view| {
            let Some(me) = weak.upgrade() else { return };
            me.web_ready.set(false);
            let crashes = me.crashes.get() + 1;
            me.crashes.set(crashes);
            if crashes > 3 {
                log("Crumb: his page keeps stopping; showing the plain orb");
                me.drop_page();
            } else {
                log("Crumb: his page stopped; loading it again");
                view.reload();
            }
        });
        web.load_uri(&format!("file://{}", folder.join("index.html").display()));
        *self.web.borrow_mut() = Some(web);
        self.set_look(Look::Crumb);
    }

    pub fn set_activity(self: &Rc<Self>, activity: Activity) {
        if self.activity.replace(activity) != activity {
            self.canvas.queue_draw();
            self.spinner.queue_draw();
            self.send_state();
            self.schedule_tick();
        }
    }

    /// A dictation was typed: he celebrates.
    pub fn delivered(&self) {
        self.delivery.set(self.delivery.get().wrapping_add(1));
        self.send_state();
    }

    /// A warning, as the Mac's wide pill; click or wait to dismiss.
    pub fn warn(self: &Rc<Self>, text: &str) {
        self.message.set_text(text);
        self.showing_message.set(Some(Instant::now()));
        self.stack.set_visible_child_name("message");
        self.place();
        self.schedule_tick();
    }

    fn dismiss_message(&self) {
        if self.showing_message.take().is_some() {
            self.stack.set_visible_child_name(if self.look.get() == Look::Crumb { "crumb" } else { "orb" });
            self.place();
        }
    }

    fn active(&self) -> bool {
        self.activity.get() != Activity::Idle
    }

    fn send_state(&self) {
        if !self.web_ready.get() {
            return;
        }
        let Some(web) = self.web.borrow().clone() else { return };
        let activity = self.activity.get();
        let level = if activity == Activity::Listening { self.level.get() } else { 0.0 };
        let gentle = gtk::Settings::default().is_some_and(|s| !s.is_gtk_enable_animations());
        let json = payload(self.active() && self.shown.get(), self.delivery.get(), gentle, level, activity == Activity::Working, activity == Activity::Listening);
        if *self.last_json.borrow() != json {
            web.run(&format!("window.updateCrumb({json})"));
            *self.last_json.borrow_mut() = json;
        }
    }

    /// One timer: 30 a second while something happens, 4 a second otherwise.
    fn schedule_tick(self: &Rc<Self>) {
        if self.tick_due.replace(true) || !self.shown.get() {
            if !self.shown.get() {
                self.tick_due.set(false);
            }
            return;
        }
        let every = if self.active() { Duration::from_millis(33) } else { Duration::from_millis(250) };
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(every, move || {
            if let Some(me) = weak.upgrade() {
                me.tick_due.set(false);
                me.tick();
                me.schedule_tick();
            }
        });
    }

    fn tick(&self) {
        let target = if self.activity.get() == Activity::Listening { (self.hooks.level)() as f64 } else { 0.0 };
        self.level.set(self.level.get() + (target - self.level.get()) * 0.5);
        if let Some(since) = self.showing_message.get() {
            if since.elapsed() > MESSAGE_FOR {
                self.dismiss_message();
            }
        }
        if self.active() {
            if self.look.get() == Look::Orb {
                self.canvas.queue_draw();
            }
            self.spinner.queue_draw();
        }
        self.send_state();
    }
}

/// What his page is told, in the exact form Caw for Mac and Android send:
/// keys sorted, the level to two decimals.
pub fn payload(active: bool, delivery: u32, gentle: bool, level: f64, processing: bool, recording: bool) -> String {
    let hundredths = (level.clamp(0.0, 1.0) * 100.0).round() as u32;
    let level = match hundredths {
        0 => "0".to_string(),
        100 => "1".to_string(),
        n => format!("0.{n:02}").trim_end_matches('0').to_string(),
    };
    format!("{{\"active\":{active},\"delivery\":{delivery},\"gentle\":{gentle},\"level\":{level},\"processing\":{processing},\"recording\":{recording},\"waiting\":false}}")
}


/// The pointer's place on screen, from Hyprland (None elsewhere, or if it doesn't answer).
fn hyprland_cursor() -> Option<(i32, i32)> {
    use std::io::{Read, Write};
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let path = dirs::runtime_dir()?.join("hypr").join(signature).join(".socket.sock");
    let mut stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(50))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(50))).ok()?;
    stream.write_all(b"j/cursorpos").ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    let v: serde_json::Value = serde_json::from_str(&reply).ok()?;
    Some((v.get("x")?.as_i64()? as i32, v.get("y")?.as_i64()? as i32))
}

#[cfg(test)]
mod tests {
    #[test]
    fn payload_matches_the_phone_apps() {
        // Caw for Android's CrumbPayloadTest form.
        assert_eq!(super::payload(true, 3, false, 0.5, false, true), r#"{"active":true,"delivery":3,"gentle":false,"level":0.5,"processing":false,"recording":true,"waiting":false}"#);
        assert_eq!(super::payload(false, 0, true, 0.0, false, false), r#"{"active":false,"delivery":0,"gentle":true,"level":0,"processing":false,"recording":false,"waiting":false}"#);
        assert!(super::payload(true, 0, false, 0.07, true, false).contains("\"level\":0.07,"));
        assert!(super::payload(true, 0, false, 2.0, true, false).contains("\"level\":1,"));
    }
}
