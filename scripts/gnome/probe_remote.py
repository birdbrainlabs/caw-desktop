"""What the RemoteDesktop portal (Caw's typing on GNOME) shows: starts a
keyboard+pointer session, dumps the dialog, presses its buttons in turn."""
import sys
from gnometest import BUS, PORTAL, PORTAL_PATH, Request, call, dump_accessibles, find_accessibles, log, press_button, windows, atspi
from gi.repository import GLib

IFACE = "org.freedesktop.portal.RemoteDesktop"
V = GLib.Variant
if sys.argv[1:2] == ["--register"]:
    try:
        call(PORTAL, PORTAL_PATH, "org.freedesktop.host.portal.Registry", "Register", V("(sa{sv})", (sys.argv[2], {})))
        log("registered", sys.argv[2])
    except GLib.Error as e:
        log("register failed:", e.message)
req = Request()
call(PORTAL, PORTAL_PATH, IFACE, "CreateSession", V("(a{sv})", ({"handle_token": V("s", req.token), "session_handle_token": V("s", "caw_rd_" + req.token)},)))
resp = req.wait(10)
log("CreateSession:", resp)
session = resp[1]["session_handle"]
req = Request()
call(PORTAL, PORTAL_PATH, IFACE, "SelectDevices", V("(oa{sv})", (session, {"handle_token": V("s", req.token), "types": V("u", 3), "persist_mode": V("u", 2)})))
log("SelectDevices:", req.wait(10))
req = Request()
call(PORTAL, PORTAL_PATH, IFACE, "Start", V("(osa{sv})", (session, "", {"handle_token": V("s", req.token)})))
resp = req.wait(5)
if resp is None:
    log("Start: waiting; windows:", windows())
    dump_accessibles()
    Atspi = atspi()
    log("buttons:", [b.get_name() for b in find_accessibles(lambda n, app: n.get_role() == Atspi.Role.BUTTON)])
    for label in ("Share", "Allow", "OK", "Continue"):
        if press_button(label):
            log("pressed", label)
            resp = req.wait(6)
            if resp is not None:
                break
log("Start:", resp)
