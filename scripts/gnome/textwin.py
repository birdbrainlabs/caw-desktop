"""A plain GTK 4 window with one text box, for the typing tests: whatever is
typed into it is written to the file given (rewritten on every change)."""

import sys

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

out = sys.argv[1]
open(out, "w").close()


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title="Caw test text")
    win.set_default_size(700, 300)
    view = Gtk.TextView()
    buf = view.get_buffer()

    def changed(b):
        with open(out, "w") as f:
            f.write(b.get_text(b.get_start_iter(), b.get_end_iter(), False))

    buf.connect("changed", changed)
    win.set_child(view)
    win.present()
    view.grab_focus()


app = Gtk.Application(application_id="dev.birdbrainlabs.CawTestText")
app.connect("activate", activate)
app.run([])
