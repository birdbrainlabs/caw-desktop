//! The Mac app: Caw Desktop.app runs the receiver with a menu bar icon and no
//! window or Dock icon (LSUIElement). The menu shows whether a phone is
//! connected, pairs a phone, turns starting at login on and off, and points to
//! the Accessibility setting until the user has allowed typing.

use crate::cgevent;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar,
    NSVariableStatusItemLength,
};
use objc2_foundation::{ns_string, NSObject, NSObjectProtocol, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use std::path::PathBuf;

const ACCESSIBILITY_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// What the menu asks of the receiver.
pub struct Hooks {
    /// Shows the pairing page.
    pub pair: Box<dyn Fn()>,
    /// The status line, e.g. "No phone connected".
    pub status: Box<dyn Fn() -> String>,
    /// Where to note that starting at login was set up on the first run.
    pub config_dir: PathBuf,
}

/// True when this binary runs as the executable of an app bundle.
pub fn in_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.ends_with("Contents/MacOS") && dir.parent().is_some_and(|c| c.join("Info.plist").is_file())))
        .unwrap_or(false)
}

struct Ivars {
    status: Retained<NSMenuItem>,
    access: Retained<NSMenuItem>,
    login: Retained<NSMenuItem>,
    hooks: Hooks,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and Controller doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CawDesktopController"]
    #[ivars = Ivars]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSMenuDelegate for Controller {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, _menu: &NSMenu) {
            self.refresh();
        }
    }

    unsafe impl NSApplicationDelegate for Controller {
        /// Opening the app again while it runs shows the pairing page.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            (self.ivars().hooks.pair)();
            false
        }
    }

    impl Controller {
        #[unsafe(method(pair:))]
        fn pair(&self, _sender: Option<&AnyObject>) {
            (self.ivars().hooks.pair)();
        }

        #[unsafe(method(openAccessibility:))]
        fn open_accessibility(&self, _sender: Option<&AnyObject>) {
            open(ACCESSIBILITY_SETTINGS);
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            let service = unsafe { SMAppService::mainAppService() };
            let result = if starts_at_login(&service) { unsafe { service.unregisterAndReturnError() } } else { unsafe { service.registerAndReturnError() } };
            if let Err(e) = result {
                eprintln!("caw-desktop: changing start at login: {}", e.localizedDescription());
            }
            // Turned on but waiting for the user's OK in System Settings → General → Login Items.
            if unsafe { service.status() } == SMAppServiceStatus::RequiresApproval {
                unsafe { SMAppService::openSystemSettingsLoginItems() };
            }
            self.refresh();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

impl Controller {
    fn refresh(&self) {
        let ivars = self.ivars();
        ivars.status.setTitle(&NSString::from_str(&(ivars.hooks.status)()));
        ivars.access.setHidden(cgevent::allowed());
        let on = starts_at_login(&*unsafe { SMAppService::mainAppService() });
        ivars.login.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    }
}

fn starts_at_login(service: &SMAppService) -> bool {
    matches!(unsafe { service.status() }, SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval)
}

fn open(target: &str) {
    match std::process::Command::new("/usr/bin/open").arg(target).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("caw-desktop: opening {target}: {e}"),
    }
}

fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>) -> Retained<NSMenuItem> {
    unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), action, ns_string!("")) }
}

/// Starting at login is on by default: the first run registers the app as a login item.
fn start_at_login_first_time(config_dir: &std::path::Path) {
    let marker = config_dir.join("login-item-set");
    if marker.exists() {
        return;
    }
    let service = unsafe { SMAppService::mainAppService() };
    if !starts_at_login(&service) {
        if let Err(e) = unsafe { service.registerAndReturnError() } {
            eprintln!("caw-desktop: starting at login: {}", e.localizedDescription());
            return;
        }
    }
    let _ = std::fs::create_dir_all(config_dir);
    let _ = std::fs::write(marker, b"");
}

/// Runs the menu bar icon on the main thread until the user quits.
pub fn run(hooks: Hooks) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("the menu bar must run on the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    start_at_login_first_time(&hooks.config_dir);

    let status = item(mtm, "No phone connected", None);
    status.setEnabled(false);
    let access = item(mtm, "Allow Caw Desktop in Privacy & Security → Accessibility", Some(sel!(openAccessibility:)));
    let login = item(mtm, "Start at login", Some(sel!(toggleLogin:)));
    let pair = item(mtm, "Pair phone…", Some(sel!(pair:)));
    let quit = item(mtm, "Quit Caw Desktop", Some(sel!(quit:)));

    let controller: Retained<Controller> = {
        let this = Controller::alloc(mtm).set_ivars(Ivars { status: status.clone(), access: access.clone(), login: login.clone(), hooks });
        unsafe { msg_send![super(this), init] }
    };
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for entry in [&status, &access] {
        menu.addItem(entry);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    for entry in [&pair, &login] {
        unsafe { entry.setTarget(Some(&controller)) };
        menu.addItem(entry);
    }
    unsafe { access.setTarget(Some(&controller)) };
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    unsafe { quit.setTarget(Some(&controller)) };
    menu.addItem(&quit);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    controller.refresh();

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        match NSImage::imageWithSystemSymbolName_accessibilityDescription(ns_string!("keyboard"), Some(ns_string!("Caw Desktop"))) {
            Some(image) => {
                image.setTemplate(true);
                button.setImage(Some(&image));
            }
            None => button.setTitle(ns_string!("Caw")),
        }
        button.setToolTip(Some(ns_string!("Caw Desktop")));
    }
    item.setMenu(Some(&menu));

    app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    app.run();
    // Kept alive until the app quits.
    drop((item, controller));
    Ok(())
}
