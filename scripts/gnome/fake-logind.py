"""A stand-in for systemd-logind on the container's system bus (run as root):
GNOME Shell won't start without org.freedesktop.login1, and containers have
no logind. One active local session, c1, for the test user. Test-only."""

import os
import sys

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

UID = int(sys.argv[1]) if len(sys.argv) > 1 else 1500
NAME = sys.argv[2] if len(sys.argv) > 2 else "tester"
SESSION = "/org/freedesktop/login1/session/c1"
USER = f"/org/freedesktop/login1/user/_{UID}"
SEAT = "/org/freedesktop/login1/seat/seat0"

XML = """
<node>
  <interface name="org.freedesktop.login1.Manager">
    <method name="GetSession"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="GetSessionByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
    <method name="GetUser"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
    <method name="GetUserByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
    <method name="GetSeat"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="ListSessions"><arg type="a(susso)" direction="out"/></method>
    <method name="ListUsers"><arg type="a(uso)" direction="out"/></method>
    <method name="CanSuspend"><arg type="s" direction="out"/></method>
    <method name="CanHibernate"><arg type="s" direction="out"/></method>
    <method name="CanPowerOff"><arg type="s" direction="out"/></method>
    <method name="CanReboot"><arg type="s" direction="out"/></method>
    <method name="CanRebootToBootLoaderMenu"><arg type="s" direction="out"/></method>
    <method name="Inhibit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="h" direction="out"/></method>
    <signal name="PrepareForSleep"><arg type="b"/></signal>
    <signal name="PrepareForShutdown"><arg type="b"/></signal>
    <property name="IdleHint" type="b" access="read"/>
    <property name="PreparingForSleep" type="b" access="read"/>
    <property name="PreparingForShutdown" type="b" access="read"/>
  </interface>
  <interface name="org.freedesktop.login1.Session">
    <method name="SetLockedHint"><arg type="b" direction="in"/></method>
    <method name="SetIdleHint"><arg type="b" direction="in"/></method>
    <method name="Activate"/>
    <method name="Lock"/>
    <method name="Unlock"/>
    <method name="TakeControl"><arg type="b" direction="in"/></method>
    <method name="ReleaseControl"/>
    <signal name="Lock"/>
    <signal name="Unlock"/>
    <property name="Id" type="s" access="read"/>
    <property name="User" type="(uo)" access="read"/>
    <property name="Name" type="s" access="read"/>
    <property name="Seat" type="(so)" access="read"/>
    <property name="Display" type="s" access="read"/>
    <property name="Type" type="s" access="read"/>
    <property name="Class" type="s" access="read"/>
    <property name="Desktop" type="s" access="read"/>
    <property name="Active" type="b" access="read"/>
    <property name="State" type="s" access="read"/>
    <property name="Remote" type="b" access="read"/>
    <property name="LockedHint" type="b" access="read"/>
    <property name="IdleHint" type="b" access="read"/>
  </interface>
  <interface name="org.freedesktop.login1.User">
    <property name="UID" type="u" access="read"/>
    <property name="Name" type="s" access="read"/>
    <property name="Display" type="(so)" access="read"/>
    <property name="Sessions" type="a(so)" access="read"/>
    <property name="State" type="s" access="read"/>
  </interface>
</node>
"""

node = Gio.DBusNodeInfo.new_for_xml(XML)
V = GLib.Variant


def manager_call(conn, sender, path, iface, method, params, inv):
    if method in ("GetSession", "GetSessionByPID"):
        inv.return_value(V("(o)", (SESSION,)))
    elif method in ("GetUser", "GetUserByPID"):
        inv.return_value(V("(o)", (USER,)))
    elif method == "GetSeat":
        inv.return_value(V("(o)", (SEAT,)))
    elif method == "ListSessions":
        inv.return_value(V("(a(susso))", ([("c1", UID, NAME, "seat0", SESSION)],)))
    elif method == "ListUsers":
        inv.return_value(V("(a(uso))", ([(UID, NAME, USER)],)))
    elif method.startswith("Can"):
        inv.return_value(V("(s)", ("no",)))
    elif method == "Inhibit":
        r, w = os.pipe()
        fds = Gio.UnixFDList.new_from_array([r])
        inv.return_value_with_unix_fd_list(V("(h)", (0,)), fds)
    else:
        inv.return_dbus_error("org.freedesktop.DBus.Error.NotSupported", method)


def manager_prop(conn, sender, path, iface, name):
    return V("b", False)


def session_call(conn, sender, path, iface, method, params, inv):
    inv.return_value(None)


SESSION_PROPS = {
    "Id": V("s", "c1"),
    "User": V("(uo)", (UID, USER)),
    "Name": V("s", NAME),
    "Seat": V("(so)", ("seat0", SEAT)),
    "Display": V("s", ""),
    "Type": V("s", "wayland"),
    "Class": V("s", "user"),
    "Desktop": V("s", "ubuntu"),
    "Active": V("b", True),
    "State": V("s", "active"),
    "Remote": V("b", False),
    "LockedHint": V("b", False),
    "IdleHint": V("b", False),
}
USER_PROPS = {
    "UID": V("u", UID),
    "Name": V("s", NAME),
    "Display": V("(so)", ("c1", SESSION)),
    "Sessions": V("a(so)", [("c1", SESSION)]),
    "State": V("s", "active"),
}


def on_bus(conn, name):
    conn.register_object("/org/freedesktop/login1", node.lookup_interface("org.freedesktop.login1.Manager"), manager_call, manager_prop, None)
    conn.register_object(SESSION, node.lookup_interface("org.freedesktop.login1.Session"), session_call, lambda *a: SESSION_PROPS.get(a[4]), None)
    conn.register_object(USER, node.lookup_interface("org.freedesktop.login1.User"), session_call, lambda *a: USER_PROPS.get(a[4]), None)


def on_name(conn, name):
    print("fake logind: owns", name, flush=True)


def on_lost(conn, name):
    print("fake logind: could not own", name, flush=True)
    loop.quit()


loop = GLib.MainLoop()
Gio.bus_own_name(Gio.BusType.SYSTEM, "org.freedesktop.login1", Gio.BusNameOwnerFlags.NONE, on_bus, on_name, on_lost)
loop.run()
