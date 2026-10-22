"""What the desktop's GlobalShortcuts portal does with Caw's hold-to-talk key:
creates a session, asks to bind Right Ctrl (and, to compare, Ctrl+Alt+K),
reports any dialog it shows (AT-SPI), accepts it, then presses the keys and
reports Activated / Deactivated. Usage: probe_shortcuts.py [--register APP_ID] [TRIGGER...]"""

import sys
import time

from gnometest import BUS, PORTAL, PORTAL_PATH, Request, call, dump_accessibles, find_accessibles, key, log, press_button, prop, pump, windows, atspi
from gi.repository import Gio, GLib

IFACE = "org.freedesktop.portal.GlobalShortcuts"
V = GLib.Variant

args = sys.argv[1:]
app_id = None
if args[:1] == ["--register"]:
    app_id = args[1]
    args = args[2:]
triggers = args or ["Control_R"]

if app_id:
    try:
        call(PORTAL, PORTAL_PATH, "org.freedesktop.host.portal.Registry", "Register", V("(sa{sv})", (app_id, {})))
        log(f"registered as {app_id}")
    except GLib.Error as e:
        log(f"Registry.Register failed: {e.message}")

log("GlobalShortcuts version:", prop(PORTAL, PORTAL_PATH, IFACE, "version"))
log("RemoteDesktop version:", prop(PORTAL, PORTAL_PATH, "org.freedesktop.portal.RemoteDesktop", "version"))

signals = []


def on_signal(_c, _s, _p, _i, name, params, _d):
    signals.append((time.monotonic(), name, params.unpack()))
    log(f"signal {name}: {params.unpack()}")


for name in ("Activated", "Deactivated", "ShortcutsChanged"):
    BUS.signal_subscribe(PORTAL, IFACE, name, PORTAL_PATH, None, Gio.DBusSignalFlags.NONE, on_signal, None)

req = Request()
try:
    call(PORTAL, PORTAL_PATH, IFACE, "CreateSession", V("(a{sv})", ({"handle_token": V("s", req.token), "session_handle_token": V("s", f"caw_session_{req.token}")},)))
except GLib.Error as e:
    log(f"CreateSession call failed: {e.message}")
    sys.exit(0)
resp = req.wait(15)
log("CreateSession response:", resp)
if not resp or resp[0] != 0:
    sys.exit(0)
session = resp[1]["session_handle"]

shortcuts = []
for i, trig in enumerate(triggers):
    shortcuts.append((f"s{i}", {"description": V("s", f"Probe {trig}"), "preferred_trigger": V("s", trig)}))

req = Request()
try:
    call(PORTAL, PORTAL_PATH, IFACE, "BindShortcuts", V("(oa(sa{sv})sa{sv})", (session, shortcuts, "", {"handle_token": V("s", req.token)})))
except GLib.Error as e:
    log(f"BindShortcuts call failed: {e.message}")
    sys.exit(0)

resp = req.wait(6)
from gnometest import calm_shell
if resp is None:
    log("BindShortcuts: no answer after 6 s; windows now:", windows())
    log("---- accessibility tree ----")
    dump_accessibles()
    log("---- end ----")
    Atspi = atspi()
    labels = [b.get_name() for b in find_accessibles(lambda n, app: n.get_role() == Atspi.Role.BUTTON)]
    log("buttons:", labels)
    for label in ("Add", "Allow", "Save", "Done", "Set", "OK", "Continue", "Apply", "Confirm"):
        if press_button(label):
            log(f"pressed {label!r}")
            resp = req.wait(6)
            if resp is not None:
                break
            log("still no answer; tree now:")
            dump_accessibles()
if resp is None:
    resp = req.wait(10)
log("BindShortcuts response:", resp)
log("shell:", calm_shell(), windows())

req = Request()
try:
    call(PORTAL, PORTAL_PATH, IFACE, "ListShortcuts", V("(oa{sv})", (session, {"handle_token": V("s", req.token)})))
    log("ListShortcuts response:", req.wait(10))
except GLib.Error as e:
    log(f"ListShortcuts failed: {e.message}")

# Press each trigger as a person would.
plan = {
    "Control_R": [["Control_R"]],
    "CTRL+ALT+k": [["Control_L", "Alt_L", "k"]],
    "LOGO+F9": [["Super_L", "F9"]],
    "F9": [["F9"]],
}
for trig in triggers + ["CTRL+ALT+k"]:
    for keys in plan.get(trig, []):
        log(f"pressing {'+'.join(keys)} for 1 s")
        before = len(signals)
        for k in keys:
            key(k, True)
            time.sleep(0.05)
        pump(1.0)
        for k in reversed(keys):
            key(k, False)
            time.sleep(0.05)
        pump(1.5)
        got = [s[1] for s in signals[before:]]
        log(f"  -> signals: {got or 'none'}")
