use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 3;
pub const DEFAULT_PORT: u16 = 47820;
pub const SERVICE_TYPE: &str = "_caw._tcp.local.";
pub const MAX_LINE: usize = 64 * 1024;

fn one() -> u32 {
    1
}

fn click() -> String {
    "click".into()
}

/// One line of newline-delimited JSON.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    PairRequest { v: u32, device_id: String, device_name: String, proof: String },
    PairOk { desktop_id: String, desktop_name: String, proof: String },
    PairError { reason: String },
    Hello {
        v: u32,
        device_id: String,
        /// Random per app start, so message ids that restart at 1 are not taken for resends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    HelloOk { desktop_name: String },
    Text { id: u64, text: String },
    Key { id: u64, key: String, #[serde(default = "one")] count: u32 },
    Ack { id: u64 },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        reason: String,
    },
    Ping,
    Pong,
    // v2: touchpad. Moves and scrolls are fire-and-forget (no id, no ack).
    PointerMove { dx: f64, dy: f64 },
    Click {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        button: String,
        #[serde(default = "click")]
        action: String,
    },
    Scroll { dx: f64, dy: f64 },
    // v3: settings the phone can view and change.
    SettingsGet { id: u64 },
    Settings {
        id: u64,
        desktop_name: String,
        desktop_version: String,
        word_fixes: std::collections::BTreeMap<String, String>,
        pointer_speed: f64,
        pointer_accel: f64,
    },
    SettingsSet {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        word_fixes: Option<std::collections::BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pointer_speed: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pointer_accel: Option<f64>,
    },
}

/// The `caw://pair?...` link shown as a QR code.
#[derive(Debug, Clone, PartialEq)]
pub struct PairUri {
    pub host: String,
    pub port: u16,
    pub code: String,
    pub name: String,
}

impl PairUri {
    pub fn to_uri(&self) -> String {
        format!(
            "caw://pair?h={}&p={}&c={}&n={}",
            self.host,
            self.port,
            self.code,
            urlencoding::encode(&self.name)
        )
    }

    pub fn parse(uri: &str) -> Result<Self> {
        let Some(query) = uri.strip_prefix("caw://pair?") else { bail!("not a caw://pair link") };
        let (mut host, mut port, mut code, mut name) = (None, None, None, String::new());
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = urlencoding::decode(v)?.into_owned();
            match k {
                "h" => host = Some(v),
                "p" => port = Some(v.parse().context("bad port")?),
                "c" => code = Some(v),
                "n" => name = v,
                _ => {}
            }
        }
        Ok(Self {
            host: host.context("missing host")?,
            port: port.unwrap_or(DEFAULT_PORT),
            code: code.context("missing code")?,
            name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_matches_spec() {
        let m = Msg::Text { id: 1, text: "Hello world. ".into() };
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"type":"text","id":1,"text":"Hello world. "}"#);
        let k: Msg = serde_json::from_str(r#"{"type":"key","id":2,"key":"backspace"}"#).unwrap();
        assert_eq!(k, Msg::Key { id: 2, key: "backspace".into(), count: 1 });
        assert_eq!(serde_json::to_string(&Msg::Ping).unwrap(), r#"{"type":"ping"}"#);
        let c: Msg = serde_json::from_str(r#"{"type":"click","button":"right"}"#).unwrap();
        assert_eq!(c, Msg::Click { id: None, button: "right".into(), action: "click".into() });
        let m: Msg = serde_json::from_str(r#"{"type":"pointer_move","dx":3,"dy":-1.5}"#).unwrap();
        assert_eq!(m, Msg::PointerMove { dx: 3.0, dy: -1.5 });
        let set: Msg = serde_json::from_str(r#"{"type":"settings_set","id":5,"pointer_speed":2.0}"#).unwrap();
        assert_eq!(set, Msg::SettingsSet { id: 5, word_fixes: None, pointer_speed: Some(2.0), pointer_accel: None });
        let e = Msg::Error { id: None, reason: "x".into() };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"type":"error","reason":"x"}"#);
    }

    #[test]
    fn pair_uri_round_trip() {
        let u = PairUri { host: "192.168.1.20".into(), port: 47820, code: "ABCD".into(), name: "my laptop".into() };
        assert_eq!(u.to_uri(), "caw://pair?h=192.168.1.20&p=47820&c=ABCD&n=my%20laptop");
        assert_eq!(PairUri::parse(&u.to_uri()).unwrap(), u);
    }
}
