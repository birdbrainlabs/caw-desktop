"""Do injected keys reach the desktop's shortcuts? Grabs accelerators directly
in the shell (Ctrl+Alt+K, and a lone Control_R), presses them, and reports
which fired, on press and on release."""
import time
from gnometest import key, log, shell_eval

setup = """(() => {
  const Meta = imports.gi.Meta;
  global._cawHits = [];
  const ids = {};
  for (const a of ['<Control><Alt>k', 'Control_R', '<Super>F9']) {
    ids[a] = global.display.grab_accelerator(a, Meta.KeyBindingFlags.NONE);
  }
  global._cawIds = ids;
  if (!global._cawSig) {
    global._cawSig = global.display.connect('accelerator-activated', (d, action) => global._cawHits.push('down ' + action));
    try { global._cawSig2 = global.display.connect('accelerator-deactivated', (d, action) => global._cawHits.push('up ' + action)); } catch (e) { global._cawHits.push('no deactivated signal: ' + e); }
  }
  return JSON.stringify(ids);
})()"""
log("grabbed:", shell_eval(setup))
for name, keys in (("<Control><Alt>k", ["Control_L", "Alt_L", "k"]), ("Control_R", ["Control_R"]), ("<Super>F9", ["Super_L", "F9"])):
    shell_eval("global._cawHits = []")
    for k in keys:
        key(k, True)
        time.sleep(0.05)
    time.sleep(0.5)
    for k in reversed(keys):
        key(k, False)
        time.sleep(0.05)
    time.sleep(0.5)
    log(f"{name}: {shell_eval('JSON.stringify(global._cawHits)')}")
