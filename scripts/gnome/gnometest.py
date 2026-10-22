"""Helpers for the headless GNOME tests: the shell's Eval (unsafe mode, from
the test extension), pressing keys as a real keyboard would, screenshots,
the portals' request/response pattern, and AT-SPI to read windows and press
dialog buttons."""

import os
import sys
import time

import gi

gi.require_version("Gio", "2.0")
gi.require_version("GLib", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

BUS = Gio.bus_get_sync(Gio.BusType.SESSION, None)
PORTAL = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"


def log(*args):
    print(*args, flush=True)


def pump(seconds):
    """Runs the GLib main loop for a while, so signals arrive."""
    ctx = GLib.MainContext.default()
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        while ctx.pending():
            ctx.iteration(False)
        time.sleep(0.02)


def call(name, path, iface, method, args=None, reply=None, timeout=30000, bus=None):
    return (bus or BUS).call_sync(name, path, iface, method, args, GLib.VariantType(reply) if reply else None, Gio.DBusCallFlags.NONE, timeout, None)


def prop(name, path, iface, prop_name, bus=None):
    try:
        return call(name, path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", (iface, prop_name)), bus=bus).unpack()[0]
    except GLib.Error as e:
        return f"error: {e.message}"


def shell_eval(js):
    ok, out = call("org.gnome.Shell", "/org/gnome/Shell", "org.gnome.Shell", "Eval", GLib.Variant("(s)", (js,))).unpack()
    return ok, out


def key(keyval_name, pressed):
    """Presses or releases a key on a virtual keyboard inside the shell: it goes
    through the same path as a real key, keybindings included."""
    js = (
        "(() => { const C = imports.gi.Clutter, G = imports.gi.GLib;"
        " if (!global._cawKbd) global._cawKbd = C.get_default_backend().get_default_seat().create_virtual_device(C.InputDeviceType.KEYBOARD_DEVICE);"
        f" global._cawKbd.notify_keyval(G.get_monotonic_time(), C.KEY_{keyval_name}, {'C.KeyState.PRESSED' if pressed else 'C.KeyState.RELEASED'});"
        " return 'ok'; })()"
    )
    return shell_eval(js)


def tap(keyval_name, hold=0.05):
    key(keyval_name, True)
    time.sleep(hold)
    key(keyval_name, False)


def screenshot(path, x=0, y=0, w=1280, h=800):
    try:
        r = call("org.gnome.Shell.Screenshot", "/org/gnome/Shell/Screenshot", "org.gnome.Shell.Screenshot", "ScreenshotArea", GLib.Variant("(iiiibs)", (x, y, w, h, False, path)))
        return r.unpack()
    except GLib.Error as e:
        return (False, e.message)


def calm_shell():
    """Out of the overview, so the desktop's shortcuts work as on a normal desktop."""
    return shell_eval("Main.overview.hide(); Main.actionMode")


def windows():
    ok, out = shell_eval("JSON.stringify(global.get_window_actors().map(a => [a.meta_window.get_title(), a.meta_window.get_wm_class(), a.meta_window.has_focus()]))")
    return out


class Request:
    """A portal request: its handle path is known before the call, so the Response signal can't be missed."""

    count = 0

    def __init__(self, bus=None):
        Request.count += 1
        self.bus = bus or BUS
        self.token = f"caw_test_{os.getpid()}_{Request.count}"
        sender = self.bus.get_unique_name()[1:].replace(".", "_")
        self.path = f"/org/freedesktop/portal/desktop/request/{sender}/{self.token}"
        self.response = None
        self.sub = self.bus.signal_subscribe(PORTAL, "org.freedesktop.portal.Request", "Response", self.path, None, Gio.DBusSignalFlags.NONE, self._on, None)

    def _on(self, _conn, _sender, _path, _iface, _signal, params, _data):
        self.response = params.unpack()

    def wait(self, seconds, while_waiting=None):
        end = time.monotonic() + seconds
        while self.response is None and time.monotonic() < end:
            pump(0.2)
            if while_waiting:
                while_waiting()
        return self.response


# ---- AT-SPI ----

def atspi():
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    return Atspi


def dump_accessibles(max_depth=14, out=sys.stdout, skip=("gnome-shell",)):
    """Every application's windows (but the shell's own), as role 'name' lines."""
    Atspi = atspi()
    desktop = Atspi.get_desktop(0)

    def walk(node, depth):
        if node is None or depth > max_depth:
            return
        try:
            role = node.get_role_name()
            name = node.get_name() or ""
            text = ""
            try:
                t = node.get_text_iface()
                if t is not None:
                    text = t.get_text(0, min(t.get_character_count(), 200))
            except Exception:
                pass
            states = []
            try:
                ss = node.get_state_set()
                for s in ("focused", "sensitive", "showing"):
                    if ss.contains(getattr(Atspi.StateType, s.upper())):
                        states.append(s)
            except Exception:
                pass
            print("  " * depth + f"{role} {name!r}" + (f" text={text!r}" if text else "") + (f" [{','.join(states)}]" if states else ""), file=out)
            for i in range(node.get_child_count()):
                walk(node.get_child_at_index(i), depth + 1)
        except Exception as e:  # nodes vanish while we walk
            print("  " * depth + f"(gone: {e})", file=out)

    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is not None and app.get_name() in skip:
            print(f"application {app.get_name()!r} (skipped)", file=out)
            continue
        walk(app, 0)
    out.flush()


def find_accessibles(pred, max_depth=16):
    Atspi = atspi()
    desktop = Atspi.get_desktop(0)
    found = []

    def walk(node, depth, app):
        if node is None or depth > max_depth:
            return
        try:
            if pred(node, app):
                found.append(node)
            for i in range(node.get_child_count()):
                walk(node.get_child_at_index(i), depth + 1, app)
        except Exception:
            pass

    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        walk(app, 0, app.get_name() if app else "")
    return found


def press_button(label, app_contains=""):
    """Does the default action of a push button whose name is `label`. True if one was pressed."""
    Atspi = atspi()
    buttons = find_accessibles(lambda n, app: n.get_role() == Atspi.Role.BUTTON and (n.get_name() or "").replace("_", "") == label and app_contains in (app or ""))
    for b in buttons:
        try:
            action = b.get_action_iface()
            if action is not None and action.do_action(0):
                return True
        except Exception as e:
            log(f"pressing {label!r}: {e}")
    return False
