//! The hold-to-talk key. Caw never reads the keyboard: the desktop's own
//! shortcut runs `caw-desktop talk start` on press and `talk stop` on release.
//! On Hyprland this finds the key the config binds to Caw now, and writes the
//! lines to bind one, in the style the config uses: Lua (Hyprland 0.55 and
//! later; with the `o.bind` helper when the config has one) or hyprland.conf.

use std::path::{Path, PathBuf};

/// The talk key when none is set: Fn on Apple keyboards, where it is a key of
/// its own (XF86Fn), Right Ctrl everywhere else.
pub fn default_talk_key() -> &'static str {
    let vendor = std::fs::read_to_string("/sys/class/dmi/id/sys_vendor").unwrap_or_default();
    if vendor.trim().starts_with("Apple") {
        "fn"
    } else {
        "ctrl_r"
    }
}

/// The keys offered in Settings: (setting, name for people).
pub const CHOICES: &[(&str, &str)] = &[("ctrl_r", "Right Ctrl"), ("alt_r", "Right Alt"), ("super_r", "Right Super"), ("fn", "Fn"), ("F13", "F13")];

/// A name for people: "Right Ctrl", "Fn", or the key as written.
pub fn describe(talk_key: &str) -> String {
    let (key, _) = hyprland_key(talk_key);
    match key.as_str() {
        "Control_R" => "Right Ctrl".into(),
        "Alt_R" => "Right Alt".into(),
        "Super_R" => "Right Super".into(),
        "XF86Fn" => "Fn".into(),
        _ => key,
    }
}

/// The Hyprland key name for a talk key, and the modifier that is down when it is released.
pub fn hyprland_key(talk_key: &str) -> (String, &'static str) {
    match talk_key.trim().to_lowercase().as_str() {
        "ctrl_r" | "right_ctrl" | "control_r" => ("Control_R".into(), "CTRL"),
        "alt_r" | "right_alt" | "right_option" => ("Alt_R".into(), "ALT"),
        "super_r" | "right_super" => ("Super_R".into(), "SUPER"),
        "fn" | "xf86fn" => ("XF86Fn".into(), ""),
        _ => (talk_key.trim().to_string(), ""),
    }
}

/// The talk key setting for a Hyprland key name, if it is one of CHOICES.
pub fn talk_key_for(hypr_keys: &str) -> Option<&'static str> {
    // "CTRL + Control_R" binds the same key as "Control_R".
    let key = hypr_keys.rsplit('+').next().unwrap_or("").trim();
    CHOICES.iter().map(|(k, _)| *k).find(|k| hyprland_key(k).0.eq_ignore_ascii_case(key))
}

pub fn on_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

pub fn hypr_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")).join("hypr")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// hyprland.lua with an `o.bind(keys, description, command, options)` helper.
    LuaHelper,
    /// hyprland.lua, plain `hl.bind`.
    Lua,
    /// hyprland.conf.
    Conf,
}

/// Which config style `dir` uses; Lua when there is none yet and Hyprland is 0.55 or later.
pub fn style(dir: &Path) -> Style {
    let lua = dir.join("hyprland.lua").is_file();
    if lua || (!dir.join("hyprland.conf").is_file() && hyprland_at_least(0, 55)) {
        let helper = config_files(dir).iter().filter(|p| p.extension().is_some_and(|e| e == "lua")).any(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains("o.bind(")));
        return if helper { Style::LuaHelper } else { Style::Lua };
    }
    Style::Conf
}

fn hyprland_at_least(major: u32, minor: u32) -> bool {
    let Ok(out) = std::process::Command::new("hyprctl").args(["version", "-j"]).stderr(std::process::Stdio::null()).output() else { return false };
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let tag = json.get("tag").and_then(|t| t.as_str()).unwrap_or("");
    let mut parts = tag.trim_start_matches('v').split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse::<u32>().ok());
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => (a, b) >= (major, minor),
        _ => false,
    }
}

/// The *.lua and *.conf files in the config folder, sorted.
fn config_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "lua" || e == "conf")).collect())
        .unwrap_or_default();
    files.sort();
    files
}

/// A key the config binds to `talk start`.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    /// As the config writes it: "XF86Fn", "CTRL + Control_R", "SUPER + F12".
    pub keys: String,
    pub file: PathBuf,
}

/// The key bound to `caw-desktop talk start` in the Hyprland config folder.
pub fn find(dir: &Path) -> Option<Bound> {
    for file in config_files(dir) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let lua = file.extension().is_some_and(|e| e == "lua");
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("--") || line.starts_with('#') || !line.contains("talk start") {
                continue;
            }
            let keys = if lua { lua_keys(line) } else { conf_keys(line) };
            if let Some(keys) = keys {
                return Some(Bound { keys, file });
            }
        }
    }
    None
}

/// `o.bind("XF86Fn", ...)` or `hl.bind("SUPER + F12", ...)`.
fn lua_keys(line: &str) -> Option<String> {
    let at = line.find("o.bind(").or_else(|| line.find("hl.bind("))?;
    let rest = &line[at..];
    let start = rest.find('"')? + 1;
    let len = rest[start..].find('"')?;
    Some(rest[start..start + len].trim().to_string()).filter(|k| !k.is_empty())
}

/// `bind = SUPER, F12, exec, ...` (any bind flags).
fn conf_keys(line: &str) -> Option<String> {
    let (name, rest) = line.split_once('=')?;
    if !name.trim().starts_with("bind") {
        return None;
    }
    let mut fields = rest.split(',');
    let mods = fields.next()?.trim();
    let key = fields.next()?.trim();
    if key.is_empty() {
        return None;
    }
    Some(if mods.is_empty() { key.to_string() } else { format!("{} + {key}", mods.split_whitespace().collect::<Vec<_>>().join(" + ")) })
}

/// Where new lines should go, for people: the file that binds Caw now, else
/// bindings.lua when there is one, else the main config file.
pub fn target_file(dir: &Path, style: Style) -> PathBuf {
    if let Some(found) = find(dir) {
        return found.file;
    }
    match style {
        Style::LuaHelper | Style::Lua if dir.join("bindings.lua").is_file() => dir.join("bindings.lua"),
        Style::LuaHelper | Style::Lua => dir.join("hyprland.lua"),
        Style::Conf => dir.join("hyprland.conf"),
    }
}

/// The lines that bind `talk_key`: press starts, release stops.
pub fn lines(style: Style, talk_key: &str, exe: &str) -> Vec<String> {
    let (key, release_mod) = hyprland_key(talk_key);
    let start = format!("{exe} talk start");
    let stop = format!("{exe} talk stop");
    // Some setups report the modifier on release, some don't; a second stop does nothing.
    let mut release_keys = vec![key.clone()];
    if !release_mod.is_empty() {
        release_keys.insert(0, format!("{release_mod} + {key}"));
    }
    let mut out = vec![format!("{} Caw hold-to-talk ({}): press starts, release stops.", if style == Style::Conf { "#" } else { "--" }, describe(talk_key))];
    match style {
        Style::LuaHelper => {
            out.push(format!("o.bind({}, \"Caw: start dictation\", {})", lua_str(&key), lua_str(&start)));
            for keys in release_keys {
                out.push(format!("o.bind({}, \"Caw: stop dictation\", {}, {{ release = true }})", lua_str(&keys), lua_str(&stop)));
            }
        }
        Style::Lua => {
            out.push(format!("hl.bind({}, hl.dsp.exec_cmd({}), {{ description = \"Caw: start dictation\" }})", lua_str(&key), lua_str(&start)));
            for keys in release_keys {
                out.push(format!("hl.bind({}, hl.dsp.exec_cmd({}), {{ description = \"Caw: stop dictation\", release = true }})", lua_str(&keys), lua_str(&stop)));
            }
        }
        Style::Conf => {
            out.push(format!("bind = , {key}, exec, {start}"));
            out.push(format!("bindr = {release_mod}, {key}, exec, {stop}"));
            if !release_mod.is_empty() {
                out.push(format!("bindr = , {key}, exec, {stop}"));
            }
        }
    }
    out
}

fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// This program, quoted for a shell command line if it needs to be.
pub fn exe() -> String {
    let path = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "caw-desktop".into());
    if path.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        path
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// For the log at start: which key talks, as far as Caw can tell.
pub fn startup_hint(talk_key: &str) -> String {
    if on_hyprland() {
        return match find(&hypr_dir()) {
            Some(b) => format!("hold {} to talk", talk_key_for(&b.keys).map(describe).unwrap_or(b.keys)),
            None => "no key is bound to talk yet; see `caw-desktop talk bindings`".into(),
        };
    }
    format!("bind {} to `caw-desktop talk start` (press) and `talk stop` (release)", describe(talk_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(hyprland_key("ctrl_r"), ("Control_R".into(), "CTRL"));
        assert_eq!(hyprland_key("alt_r"), ("Alt_R".into(), "ALT"));
        assert_eq!(hyprland_key("code:472"), ("code:472".into(), ""));
        assert_eq!(hyprland_key("Alt_R"), ("Alt_R".into(), "ALT"));
        assert_eq!(hyprland_key("Control_R"), ("Control_R".into(), "CTRL"));
        assert_eq!(hyprland_key("XF86Fn"), ("XF86Fn".into(), ""));
        assert_eq!(hyprland_key("fn"), ("XF86Fn".into(), ""));
        assert_eq!(describe("fn"), "Fn");
        assert_eq!(describe("ctrl_r"), "Right Ctrl");
        assert_eq!(talk_key_for("XF86Fn"), Some("fn"));
        assert_eq!(talk_key_for("CTRL + Control_R"), Some("ctrl_r"));
        assert_eq!(talk_key_for("SUPER + F12"), None);
    }

    #[test]
    fn lua_lines() {
        let l = lines(Style::LuaHelper, "fn", "/opt/caw/caw-desktop");
        assert_eq!(l[1], r#"o.bind("XF86Fn", "Caw: start dictation", "/opt/caw/caw-desktop talk start")"#);
        assert_eq!(l[2], r#"o.bind("XF86Fn", "Caw: stop dictation", "/opt/caw/caw-desktop talk stop", { release = true })"#);
        assert_eq!(l.len(), 3);
        let l = lines(Style::LuaHelper, "ctrl_r", "caw-desktop");
        assert_eq!(l[2], r#"o.bind("CTRL + Control_R", "Caw: stop dictation", "caw-desktop talk stop", { release = true })"#);
        assert_eq!(l[3], r#"o.bind("Control_R", "Caw: stop dictation", "caw-desktop talk stop", { release = true })"#);
        let l = lines(Style::Lua, "fn", "caw-desktop");
        assert_eq!(l[1], r#"hl.bind("XF86Fn", hl.dsp.exec_cmd("caw-desktop talk start"), { description = "Caw: start dictation" })"#);
        let l = lines(Style::Conf, "ctrl_r", "caw-desktop");
        assert_eq!(l[1..], ["bind = , Control_R, exec, caw-desktop talk start", "bindr = CTRL, Control_R, exec, caw-desktop talk stop", "bindr = , Control_R, exec, caw-desktop talk stop"]);
    }

    #[test]
    fn finds_the_bound_key() {
        let dir = std::env::temp_dir().join(format!("caw-binding-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hyprland.lua"), "require(\"x\")\n").unwrap();
        std::fs::write(dir.join("bindings.lua"), "-- o.bind(\"SUPER + H\", nil, \"caw talk start\")\no.bind(\"SUPER + B\", \"Browser\", \"browser\")\n").unwrap();
        assert_eq!(find(&dir), None);
        assert_eq!(style(&dir), Style::LuaHelper);
        assert_eq!(target_file(&dir, Style::LuaHelper), dir.join("bindings.lua"));
        std::fs::write(dir.join("bindings.lua"), "o.bind(\"XF86Fn\", \"Caw: start dictation\", \"/x/caw-desktop talk start\")\n").unwrap();
        assert_eq!(find(&dir), Some(Bound { keys: "XF86Fn".into(), file: dir.join("bindings.lua") }));
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(conf_keys("bind = SUPER SHIFT, F12, exec, caw-desktop talk start").as_deref(), Some("SUPER + SHIFT + F12"));
        assert_eq!(conf_keys("bind = , Control_R, exec, caw-desktop talk start").as_deref(), Some("Control_R"));
        assert_eq!(lua_keys(r#"hl.bind("SUPER + F12", hl.dsp.exec_cmd("caw-desktop talk start"))"#).as_deref(), Some("SUPER + F12"));
    }
}
