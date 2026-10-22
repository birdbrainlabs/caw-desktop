"""The installed app on a headless GNOME session (run inside session.sh, as the
test user). Starts `caw-desktop app` beside a plain text window and a fake
microphone, answers the desktop's dialogs as a person would (and records what
they said), then checks: the tray icon and its menu, the icon while listening,
the talk key through the GlobalShortcuts portal, dictation typed into the
text window, the phone paired before the upgrade, and the Pair a Phone
window. Prints RESULT lines; exits 1 if a required check failed.
Usage: app_test.py <folder with test_wavs>"""

import json
import os
import re
import subprocess
import sys
import threading
import time
import wave

from gi.repository import Gio, GLib

from gnometest import BUS, call, calm_shell, dump_accessibles, find_accessibles, atspi, key, log, press_button, switch_on, prop, MutterKeyboard, texts_in_window, pump, screenshot, shell_eval, windows

here = os.path.dirname(os.path.abspath(__file__))
LOGS = os.environ["LOGS"]
dist = sys.argv[1]
wavs = os.path.join(dist, "target", "model", "test_wavs")
APP_LOG = f"{LOGS}/app.log"
TYPED = f"{LOGS}/typed.txt"
failed = []


def result(name, status, detail=""):
    print(f"RESULT {name}: {status}" + (f" ({detail})" if detail else ""), flush=True)
    if status == "FAIL":
        failed.append(name)


def app_log():
    try:
        return open(APP_LOG, errors="replace").read()
    except OSError:
        return ""


def wait_for(pred, seconds, step=0.25):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        v = pred()
        if v:
            return v
        pump(step)
    return pred()


def words(text):
    return re.findall(r"[a-z']+", text.lower())


def overlap(heard, expected):
    h, e = words(heard), words(expected)
    if not e:
        return 0.0
    hs = set(h)
    return sum(1 for w in e if w in hs) / len(e)


def transcript(name):
    for line in open(os.path.join(wavs, "trans.txt")):
        if line.startswith(name + " "):
            return line.split(" ", 1)[1].strip()
    return ""


# ---- a fake microphone: a PipeWire source fed from a FIFO in real time ----

FIFO = "/tmp/caw-test-mic.fifo"
subprocess.run(["pactl", "load-module", "module-pipe-source", "source_name=caw_test_mic", f"file={FIFO}", "format=s16le", "rate=16000", "channels=1"], check=False)
subprocess.run(["pactl", "set-default-source", "caw_test_mic"], check=False)
print(subprocess.run(["pactl", "info"], capture_output=True, text=True).stdout, flush=True)


class Mic(threading.Thread):
    """Writes silence, or a WAV file's samples, 100 ms at a time."""

    def __init__(self):
        super().__init__(daemon=True)
        self.queue = b""
        self.lock = threading.Lock()

    def play(self, path):
        with wave.open(path) as w:
            assert w.getframerate() == 16000 and w.getnchannels() == 1 and w.getsampwidth() == 2, path
            data = w.readframes(w.getnframes())
        with self.lock:
            self.queue += data
        return len(data) / 32000

    def run(self):
        fd = None
        chunk = 3200
        next_at = time.monotonic()
        while True:
            if fd is None:
                try:
                    fd = os.open(FIFO, os.O_WRONLY | os.O_NONBLOCK)
                except OSError:
                    time.sleep(0.2)
                    continue
            with self.lock:
                data, self.queue = self.queue[:chunk], self.queue[chunk:]
            data = data.ljust(chunk, b"\0")
            try:
                os.write(fd, data)
            except BlockingIOError:
                pass
            except OSError:
                fd = None
            next_at += 0.1
            time.sleep(max(0, next_at - time.monotonic()))


mic = Mic()
mic.start()

# ---- the text window, then the app ----

calm_shell()
textwin = subprocess.Popen(["python3", os.path.join(here, "textwin.py"), TYPED], stdout=open(f"{LOGS}/textwin.log", "w"), stderr=subprocess.STDOUT)
wait_for(lambda: "Caw test text" in windows(), 20)
log("windows:", windows())

FOCUS_TEXT = "(() => { const a = global.get_window_actors().find(a => a.meta_window.get_title() == 'Caw test text'); if (!a) return 'none'; a.meta_window.activate(global.get_current_time()); return 'ok'; })()"

# ---- which way of pressing keys reaches the desktop's shortcuts here ----
# The portal binds the talk key through the shell's GrabAccelerators, so grab
# two keys the same way and press them two ways: the shell's virtual keyboard,
# and mutter's remote-desktop keyboard (what GNOME's remote desktop uses).
shell_hits = []


def on_shell(_c, _s, _p, _i, name, params, _d):
    shell_hits.append((name, params.unpack()[0]))


for sig in ("AcceleratorActivated", "AcceleratorDeactivated"):
    BUS.signal_subscribe("org.gnome.Shell", "org.gnome.Shell", sig, "/org/gnome/Shell", None, Gio.DBusSignalFlags.NONE, on_shell, None)
try:
    mutter_kbd = MutterKeyboard()
except GLib.Error as e:
    mutter_kbd = None
    result("mutter-keyboard", "INFO", f"not available: {e.message}")
ways = {"shell virtual keyboard": key}
if mutter_kbd:
    ways["mutter remote-desktop keyboard"] = mutter_kbd.key
try:
    grabbed = call("org.gnome.Shell", "/org/gnome/Shell", "org.gnome.Shell", "GrabAccelerators", GLib.Variant("(a(suu))", ([("<Control><Alt>j", 1 | 2, 0), ("Control_R", 1 | 2, 0)],))).unpack()[0]
except GLib.Error as e:
    grabbed = []
    result("shell-grab", "INFO", e.message)
press_with = None
for (accel, keys), action in zip((("<Control><Alt>j", ["Control_L", "Alt_L", "j"]), ("Control_R", ["Control_R"])), grabbed):
    for way, press in ways.items():
        shell_hits.clear()
        for k in keys:
            press(k, True)
            pump(0.05)
        pump(0.6)
        for k in reversed(keys):
            press(k, False)
            pump(0.05)
        pump(0.8)
        got = [n.replace("Accelerator", "") for n, a in shell_hits if a == action]
        result(f"shell-shortcut {accel} by {way}", "INFO", ", ".join(got) or "nothing")
        if accel == "Control_R" and "Activated" in got and press_with is None:
            press_with = press
if grabbed:
    call("org.gnome.Shell", "/org/gnome/Shell", "org.gnome.Shell", "UngrabAccelerators", GLib.Variant("(au)", (list(grabbed),)))
# Do the injected keys reach a focused window at all?
shell_eval(FOCUS_TEXT)
pump(0.5)
for way, press in ways.items():
    start = open(TYPED).read()
    press("x", True)
    press("x", False)
    pump(1)
    result(f"typing-into-window by {way}", "INFO", "reached the window" if len(open(TYPED).read()) > len(start) else "did not reach the window")
press_with = press_with or (mutter_kbd.key if mutter_kbd else key)

app = subprocess.Popen(["caw-desktop", "app"], stdout=open(APP_LOG, "w"), stderr=subprocess.STDOUT, env=dict(os.environ, RUST_BACKTRACE="1"))

# The desktop's questions: typing (remote desktop) and the talk key (global shortcuts).
Atspi = atspi()
seen = set()
# Each: switches a person turns on, then the button they press.
answers = {
    "xdg-desktop-portal-gnome": (["Allow Remote Interaction"], ["Share"]),
    "gnome-control-center-global-shortcuts-provider": ([], ["Add"]),
}


def dialog_texts(app_name):
    labels = find_accessibles(lambda n, a: a == app_name and n.get_role_name() in ("label", "frame", "dialog", "button", "push button", "check box", "switch", "toggle button"))
    return [f"{n.get_role_name()}:{n.get_name()}" for n in labels if n.get_name()]


def answer_dialogs():
    for app_name, (switches, buttons) in answers.items():
        texts = dialog_texts(app_name)
        if not texts:
            continue
        sig = (app_name, tuple(texts))
        if sig not in seen:
            seen.add(sig)
            log(f"DIALOG from {app_name}: {texts}")
        for sw in switches:
            if switch_on(sw, app_name):
                log(f"turned on {sw!r} in {app_name}")
                pump(0.5)
        for b in buttons:
            if press_button(b, app_name):
                log(f"pressed {b!r} in {app_name}")
                time.sleep(0.5)


def settled():
    text = app_log()
    typing = "typing and mouse through" in text
    talk = re.search(r"talk key: (Bound|Declined|Unavailable|the desktop answered)", text) or "talk key: Unavailable" in text
    return typing and (talk or app.poll() is not None)


end = time.monotonic() + 90
while time.monotonic() < end and not settled():
    answer_dialogs()
    pump(0.5)
pump(2)
answer_dialogs()
log("---- app log so far ----")
print(app_log(), flush=True)
log("---- end ----")

if app.poll() is not None:
    result("app-starts", "FAIL", f"exited with {app.returncode}")
    sys.exit(1)
result("app-starts", "PASS", "running after its start-up questions")
m = re.search(r"typing and mouse through (.*)", app_log())
result("typing-method", "INFO", m.group(1) if m else "unknown")
m = re.findall(r"talk key: (.*)", app_log())
result("talk-key-portal", "INFO", " | ".join(m) if m else "no talk key messages")

# ---- the tray ----

def tray_items():
    try:
        return prop("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems")
    except Exception as e:
        return f"error: {e}"


items = wait_for(lambda: [i for i in (tray_items() if isinstance(tray_items(), list) else []) if i], 20)
ours = None
for item in items or []:
    name, _, path = item.partition("/")
    path = "/" + path if path else "/StatusNotifierItem"
    if prop(name, path, "org.kde.StatusNotifierItem", "Id") == "caw-desktop":
        ours = (name, path)
result("tray-registers", "PASS" if ours else "FAIL", f"items: {items}")


def icon_name():
    return prop(ours[0], ours[1], "org.kde.StatusNotifierItem", "IconName") if ours else None


def panel_box():
    js = """(() => { const out = []; for (const k of Object.keys(Main.panel.statusArea)) { if (!k.toLowerCase().includes('appindicator')) continue; const a = Main.panel.statusArea[k]; const [x, y] = a.get_transformed_position(); const [w, h] = a.get_transformed_size(); out.push([k, x, y, w, h, a.visible]); } return JSON.stringify(out); })()"""
    ok, out = shell_eval(js)
    try:
        v = json.loads(out)  # Eval answers in JSON, and the script's answer is JSON too
        return json.loads(v) if isinstance(v, str) else v
    except Exception:
        return out


boxes = panel_box()
log("panel indicators:", boxes)
result("tray-shown-in-panel", "PASS" if isinstance(boxes, list) and any(b[5] for b in boxes) else "FAIL", str(boxes))

menu_labels = []
if ours:
    menu_path = prop(ours[0], ours[1], "org.kde.StatusNotifierItem", "Menu")
    layout = call(ours[0], menu_path, "com.canonical.dbusmenu", "GetLayout", GLib.Variant("(iias)", (0, -1, []))).unpack()[1]
    ids = {}

    def walk(node):
        nid, props, children = node
        if "label" in props:
            label = props["label"]
            enabled = props.get("enabled", True)
            menu_labels.append((label, enabled))
            ids[label] = nid
        for c in children:
            walk(c)

    walk(layout)
log("tray menu:", menu_labels)
actions = [label for label, enabled in menu_labels if enabled]
expected = ["Dictation", "Let phones type on this computer", "Word fixes…", "Pair a phone…", "Settings…", "Quit"]
result("tray-menu", "PASS" if actions == expected else "FAIL", f"{actions}")
result("tray-status-lines", "INFO", "; ".join(label.replace("__", "_") for label, enabled in menu_labels if not enabled))

# ---- listening: the talk key, the icon, dictation, typing ----

def panel_shot(name):
    if not isinstance(boxes, list) or not boxes:
        return None
    _, x, y, w, h, _ = boxes[0]
    path = f"{LOGS}/{name}.png"
    ok, _ = screenshot(path, int(x), int(y), max(1, int(w)), max(1, int(h)))
    return path if ok else None


def pixels(path):
    from gi.repository import GdkPixbuf
    pb = GdkPixbuf.Pixbuf.new_from_file(path)
    return pb.get_width(), pb.get_height(), pb.get_n_channels(), pb.get_rowstride(), pb.get_pixels()


def changed_pixels(a, b):
    wa, ha, na, ra, pa = pixels(a)
    wb, hb, nb, rb, pb = pixels(b)
    if (wa, ha) != (wb, hb):
        return -1
    n = 0
    for y in range(ha):
        for x in range(wa):
            i, j = y * ra + x * na, y * rb + x * nb
            if sum(abs(pa[i + c] - pb[j + c]) for c in range(3)) > 60:
                n += 1
    return n


idle_icon = icon_name()
idle_shot = panel_shot("tray-idle")
shell_eval(FOCUS_TEXT)
pump(0.5)

expected_text = transcript("0.wav")
by_key = "talk key: the desktop answered" in app_log() or "talk key: Bound" in app_log()
held = False
if by_key:
    press_with("Control_R", True)
    held = True
    if wait_for(lambda: "talk key: pressed" in app_log(), 3):
        result("talk-key-press", "PASS", "the desktop told Caw Right Ctrl went down")
    else:
        result("talk-key-press", "INFO", "Right Ctrl went down, but the desktop sent Caw nothing")
if not (by_key and "talk key: pressed" in app_log()):
    # So the rest still gets tested: start as the desktop shortcut would.
    if held:
        press_with("Control_R", False)
        held = False
    log("talk start:", subprocess.run(["caw-desktop", "talk", "start"], capture_output=True, text=True).stdout.strip())

listening = wait_for(lambda: icon_name() != idle_icon and icon_name(), 3)
listen_icon = icon_name()
listen_shot = panel_shot("tray-listening")
secs = mic.play(os.path.join(wavs, "0.wav"))
time.sleep(secs + 0.8)
if held:
    press_with("Control_R", False)
    if wait_for(lambda: "talk key: released" in app_log(), 3):
        result("talk-key-release", "PASS", "the desktop told Caw Right Ctrl came up")
    else:
        result("talk-key-release", "INFO", "Right Ctrl came up, but the desktop sent Caw nothing; stopping by command")
        subprocess.run(["caw-desktop", "talk", "stop"])
else:
    subprocess.run(["caw-desktop", "talk", "stop"])

result("tray-icon-while-listening", "PASS" if listen_icon and listen_icon != idle_icon else "FAIL", f"{idle_icon} -> {listen_icon}")
if idle_shot and listen_shot:
    n = changed_pixels(idle_shot, listen_shot)
    result("tray-icon-visibly-changes", "PASS" if n > 10 else "FAIL", f"{n} pixels of the panel icon changed")
else:
    result("tray-icon-visibly-changes", "FAIL", "no screenshot")
wait_for(lambda: icon_name() == idle_icon, 30)

# Caw types through the desktop a key at a time: wait for all of it.
wait_for(lambda: overlap(open(TYPED).read(), expected_text) >= 0.8, 60)
pump(1)
typed = open(TYPED).read()
score = overlap(typed, expected_text)
result("dictation-typed-in-window", "PASS" if score >= 0.8 else "FAIL", f"typed {typed!r}; {score:.0%} of the expected words")

# ---- the phone paired before the upgrade ----

devices = subprocess.run(["caw-desktop", "devices"], capture_output=True, text=True).stdout
result("pairing-kept-after-upgrade", "PASS" if "Fake phone" in devices else "FAIL", devices.strip().replace("\n", "; "))
result("old-receiver-handed-over", "PASS" if "stopped the receiver that was already running" in app_log() else "FAIL", "")
shell_eval(FOCUS_TEXT)
pump(0.5)
before = open(TYPED).read()
phone_env = dict(os.environ, XDG_CONFIG_HOME=os.path.expanduser("~/phone"))
sent = subprocess.run(["/tmp/receiver/caw-fake-phone", "send", "Phone text after the upgrade. "], capture_output=True, text=True, env=phone_env)
log("fake phone:", sent.returncode, sent.stdout.strip(), sent.stderr.strip())
got = wait_for(lambda: "Phone text after the upgrade" in open(TYPED).read(), 15)
result("phone-types-after-upgrade", "PASS" if sent.returncode == 0 and got else "FAIL", f"phone exit {sent.returncode}; window now {open(TYPED).read()[len(before):]!r}")

# ---- Pair a Phone: the app's own window, from the menu and from `pair --window` ----

subprocess.run(["caw-desktop", "pair", "--window"], timeout=30)
shown = wait_for(lambda: "Pair a Phone" in windows(), 10)
code = wait_for(lambda: any(t.startswith("This code works for") for t in texts_in_window("Pair a Phone")), 10)
result("pair-window", "PASS" if shown and code else "FAIL", f"window shown: {bool(shown)}; it says: {texts_in_window('Pair a Phone')}")

# Settings from the tray menu.
if ours and "Settings…" in ids:
    call(ours[0], menu_path, "com.canonical.dbusmenu", "Event", GLib.Variant("(isvu)", (ids["Settings…"], "clicked", GLib.Variant("s", ""), 0)))
    shown = wait_for(lambda: "Caw Settings" in windows(), 10)
    wait_for(lambda: texts_in_window("Caw Settings"), 5)
    result("settings-window", "PASS" if shown else "FAIL", f"it says: {texts_in_window('Caw Settings')}")

# ---- quit ----
app.terminate()
try:
    app.wait(10)
    result("app-quits", "PASS", f"exit {app.returncode}")
except subprocess.TimeoutExpired:
    app.kill()
    result("app-quits", "FAIL", "still running 10 s after SIGTERM")
log("---- app log ----")
print(app_log(), flush=True)
textwin.terminate()
if failed:
    log("FAILED:", failed)
    sys.exit(1)
