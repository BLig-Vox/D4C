use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};
#[derive(Clone, Debug)]
pub struct Connection {
    pub name: String,
    pub url: String,
}
#[derive(Clone)]
pub struct Settings {
    pub connections: Vec<Connection>,
    pub selected: usize,
    pub desktop: bool,
    pub sound: bool,
    pub workspace_urls: std::collections::BTreeMap<String, String>,
    pub model_defaults: std::collections::BTreeMap<String, (String, String)>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: vec![Connection {
                name: "Codex".into(),
                url: String::new(),
            }],
            selected: 0,
            desktop: true,
            sound: true,
            workspace_urls: Default::default(),
            model_defaults: Default::default(),
        }
    }
}
impl Settings {
    pub fn load() -> Self {
        Self::parse(&read("settings.json"))
    }
    pub fn parse(v: &Value) -> Self {
        let mut s = Self::default();
        let connections: Vec<_> = v["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .take(12)
            .filter_map(|v| {
                let url = v["url"].as_str()?;
                let name = v["name"].as_str()?;
                valid_url(url).then(|| Connection {
                    name: name.chars().take(60).collect(),
                    url: url.into(),
                })
            })
            .collect();
        if !connections.is_empty() {
            s.connections = connections;
        }
        s.selected = (v["selected"].as_u64().unwrap_or(0) as usize).min(s.connections.len() - 1);
        s.desktop = v["desktop"].as_bool().unwrap_or(true);
        s.sound = v["sound"].as_bool().unwrap_or(true);
        if let Some(urls) = v["workspaceUrls"].as_object() {
            for (gateway, url) in urls.iter().take(12) {
                if valid_url(gateway) {
                    if let Some(url) = url.as_str().filter(|u| valid_url(u)) {
                        s.workspace_urls.insert(gateway.clone(), url.into());
                    }
                }
            }
        }
        if let Some(defaults) = v["modelDefaults"].as_object() {
            for (url, value) in defaults.iter().take(12) {
                if valid_url(url) {
                    if let (Some(model), Some(thinking)) =
                        (value["model"].as_str(), value["thinking"].as_str())
                    {
                        if model.len() <= 256
                            && thinking.len() <= 64
                            && !thinking.chars().any(char::is_control)
                        {
                            s.model_defaults
                                .insert(url.clone(), (model.into(), thinking.into()));
                        }
                    }
                }
            }
        }
        s
    }
    pub fn defaults(&self, url: &str) -> (String, String) {
        self.model_defaults
            .get(url)
            .cloned()
            .unwrap_or((String::new(), "medium".into()))
    }
    pub fn value(&self) -> Value {
        json!({"workspaceUrls":self.workspace_urls,"connections":self.connections.iter().map(|c|json!({"name":c.name,"url":c.url})).collect::<Vec<_>>(),"selected":self.selected,"desktop":self.desktop,"sound":self.sound,"modelDefaults":self.model_defaults.iter().map(|(u,(m,t))|(u.clone(),json!({"model":m,"thinking":t}))).collect::<serde_json::Map<_,_>>()})
    }
    pub fn save(&self) -> io::Result<()> {
        write("settings.json", &self.value())
    }
    pub fn current(&self) -> &Connection {
        &self.connections[self.selected.min(self.connections.len() - 1)]
    }
}
pub fn valid_url(url: &str) -> bool {
    // No credentials, fragments or URL query tokens in saved connections.
    url.starts_with("wss://")
        && url.len() < 2048
        && !url[6..].is_empty()
        && !url.chars().any(char::is_whitespace)
        && !url.contains(['@', '?', '#'])
}
fn dir() -> io::Result<PathBuf> {
    crate::platform::config_directory()
}
pub fn read(name: &str) -> Value {
    dir()
        .ok()
        .and_then(|d| fs::read(d.join(name)).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}
pub fn write(name: &str, v: &Value) -> io::Result<()> {
    let d = dir()?;
    fs::create_dir_all(&d)?;
    let temp = d.join(format!("{name}.tmp"));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&temp)?;
    file.write_all(&serde_json::to_vec(v)?)?;
    file.sync_all()?;
    fs::rename(temp, d.join(name))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_defaults_survive_roundtrip_and_are_connection_scoped() {
        let mut s = Settings::default();
        s.model_defaults.insert(
            "wss://one.example".into(),
            ("openai/gpt-6-luna".into(), "high".into()),
        );
        let restored = Settings::parse(&s.value());
        assert_eq!(
            restored.defaults("wss://one.example"),
            ("openai/gpt-6-luna".into(), "high".into())
        );
        assert_ne!(
            restored.defaults("wss://two.example"),
            restored.defaults("wss://one.example")
        );
    }
    #[test]
    fn settings_round_trip_and_no_tokens() {
        let mut s = Settings::default();
        s.connections.push(Connection {
            name: "Second".into(),
            url: "wss://second.example".into(),
        });
        s.selected = 1;
        let mut v = s.value();
        v["token"] = json!("secret");
        let restored = Settings::parse(&v);
        assert_eq!(restored.current().name, "Second");
        assert!(!restored.value().to_string().contains("secret"));
    }
    #[test]
    fn reject_credential_urls() {
        for u in [
            "ws://host",
            "wss://user:pass@host",
            "wss://host?token=x",
            "wss://",
            "wss://bad host",
        ] {
            assert!(!valid_url(u));
        }
        assert!(valid_url("wss://server.example:443/gateway"));
    }
}
