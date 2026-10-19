//! WebKitGTK 6.0, loaded when Crumb is first shown rather than linked, so the
//! app runs (with the plain orb) on systems without it. Only the few calls
//! Crumb's page needs; everything else goes through GObject properties and signals.

use gtk::glib;
use gtk::glib::translate::{FromGlib, FromGlibPtrFull, ToGlibPtr};
use gtk::prelude::*;
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::OnceLock;

type GType = usize;

#[repr(C)]
struct Rgba {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

struct Api {
    web_view_get_type: unsafe extern "C" fn() -> GType,
    network_session_new_ephemeral: unsafe extern "C" fn() -> *mut glib::gobject_ffi::GObject,
    load_uri: unsafe extern "C" fn(*mut c_void, *const c_char),
    reload: unsafe extern "C" fn(*mut c_void),
    #[allow(clippy::type_complexity)]
    evaluate_javascript: unsafe extern "C" fn(*mut c_void, *const c_char, isize, *const c_char, *const c_char, *mut c_void, *mut c_void, *mut c_void),
    set_background_color: unsafe extern "C" fn(*mut c_void, *const Rgba),
    context_get_default: unsafe extern "C" fn() -> *mut c_void,
    add_path_to_sandbox: unsafe extern "C" fn(*mut c_void, *const c_char, c_int),
}

// Plain function pointers into a library that is never unloaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

const LIBRARY: &str = "libwebkitgtk-6.0.so.4";

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    let name = CString::new(LIBRARY).unwrap();
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
    if handle.is_null() {
        return Err(format!("{LIBRARY} isn't installed (WebKitGTK 6.0)"));
    }
    let sym = |name: &str| -> Result<*mut c_void, String> {
        let c = CString::new(name).unwrap();
        let p = unsafe { libc::dlsym(handle, c.as_ptr()) };
        if p.is_null() {
            Err(format!("{LIBRARY} has no {name}"))
        } else {
            Ok(p)
        }
    };
    unsafe {
        Ok(Api {
            web_view_get_type: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_view_get_type")?),
            network_session_new_ephemeral: std::mem::transmute::<*mut c_void, _>(sym("webkit_network_session_new_ephemeral")?),
            load_uri: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_view_load_uri")?),
            reload: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_view_reload")?),
            evaluate_javascript: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_view_evaluate_javascript")?),
            set_background_color: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_view_set_background_color")?),
            context_get_default: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_context_get_default")?),
            add_path_to_sandbox: std::mem::transmute::<*mut c_void, _>(sym("webkit_web_context_add_path_to_sandbox")?),
        })
    }
}

/// A WebKitWebView: transparent, with an ephemeral session (nothing is kept
/// on disk), allowed to read only `folder`.
#[derive(Clone)]
pub struct WebView {
    pub widget: gtk::Widget,
}

impl WebView {
    pub fn new(folder: &std::path::Path) -> Result<Self, String> {
        let api = api()?;
        // Must happen before the first web process starts; later calls are refused harmlessly.
        let path = CString::new(folder.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        unsafe { (api.add_path_to_sandbox)((api.context_get_default)(), path.as_ptr(), 1) };
        let type_ = unsafe { glib::Type::from_glib((api.web_view_get_type)()) };
        let session: glib::Object = unsafe { glib::Object::from_glib_full((api.network_session_new_ephemeral)()) };
        let object = glib::Object::builder_with_type(type_).property("network-session", &session).build();
        let widget = object.downcast::<gtk::Widget>().map_err(|_| "WebKit's view isn't a widget".to_string())?;
        let view = Self { widget };
        unsafe { (api.set_background_color)(view.ptr(), &Rgba { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 }) };
        if let Some(settings) = view.settings() {
            for (name, value) in [("enable-developer-extras", false), ("enable-back-forward-navigation-gestures", false), ("enable-media", false)] {
                if settings.find_property(name).is_some() {
                    settings.set_property(name, value);
                }
            }
        }
        Ok(view)
    }

    fn ptr(&self) -> *mut c_void {
        let p: *mut gtk::ffi::GtkWidget = self.widget.to_glib_none().0;
        p as *mut c_void
    }

    fn settings(&self) -> Option<glib::Object> {
        // WebKitGTK 6.0 makes "settings" write-only; reading it panics.
        let spec = self.widget.find_property("settings")?;
        if !spec.flags().contains(glib::ParamFlags::READABLE) {
            return None;
        }
        self.widget.property::<Option<glib::Object>>("settings")
    }

    pub fn load_uri(&self, uri: &str) {
        let Ok(api) = api() else { return };
        let uri = CString::new(uri).unwrap_or_default();
        unsafe { (api.load_uri)(self.ptr(), uri.as_ptr()) };
    }

    pub fn reload(&self) {
        if let Ok(api) = api() {
            unsafe { (api.reload)(self.ptr()) };
        }
    }

    /// Runs `script` in the page, without waiting for a result.
    pub fn run(&self, script: &str) {
        let Ok(api) = api() else { return };
        let Ok(script) = CString::new(script) else { return };
        let null = std::ptr::null_mut();
        unsafe { (api.evaluate_javascript)(self.ptr(), script.as_ptr(), -1, std::ptr::null(), std::ptr::null(), null, null, null) };
    }

    pub fn is_loading(&self) -> bool {
        self.widget.property::<bool>("is-loading")
    }

    pub fn title(&self) -> String {
        self.widget.property::<Option<String>>("title").unwrap_or_default()
    }

    // The handlers get the view as an argument, so they hold no reference to it:
    // dropping the widget then really ends WebKit's processes.
    pub fn connect_loading_changed(&self, f: impl Fn(&WebView) + 'static) {
        self.widget.connect_notify_local(Some("is-loading"), move |w, _| f(&WebView { widget: w.clone() }));
    }

    pub fn connect_title_changed(&self, f: impl Fn(&WebView) + 'static) {
        self.widget.connect_notify_local(Some("title"), move |w, _| f(&WebView { widget: w.clone() }));
    }

    /// The page's process stopped (crashed, or was killed for memory).
    pub fn connect_crashed(&self, f: impl Fn(&WebView) + 'static) {
        self.widget.connect_local("web-process-terminated", false, move |args| {
            if let Some(widget) = args.first().and_then(|v| v.get::<gtk::Widget>().ok()) {
                f(&WebView { widget });
            }
            None
        });
    }
}
