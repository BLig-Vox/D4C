//! Inactive server catalogs carry metadata only; the selected server owns chat/VNC.
use crate::{gateway, preferences};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Session {
    pub key: String,
    pub title: String,
    pub running: bool,
}
pub struct Server {
    pub sessions: Vec<Session>,
    pub expanded: bool,
    pub online: bool,
    pub status: String,
    pub selected: Option<String>,
    link: Option<gateway::Gateway>,
}
impl Default for Server {
    fn default() -> Self {
        Self {
            sessions: vec![],
            expanded: true,
            online: false,
            status: "Connecting…".into(),
            selected: None,
            link: None,
        }
    }
}
pub struct Catalogs {
    pub rows: BTreeMap<String, Server>,
    dirty: bool,
    last_save: Instant,
}
impl Default for Catalogs {
    fn default() -> Self {
        let mut rows = BTreeMap::new();
        for item in preferences::read("server-catalogs.json")
            .as_array()
            .into_iter()
            .flatten()
            .take(12)
        {
            let Some(url) = item["url"]
                .as_str()
                .filter(|url| preferences::valid_url(url))
            else {
                continue;
            };
            let mut row = Server::default();
            row.expanded = item["expanded"].as_bool().unwrap_or(true);
            row.selected = item["selected"].as_str().map(str::to_owned);
            row.sessions = parse_sessions(&item["sessions"]);
            rows.insert(url.into(), row);
        }
        Self {
            rows,
            dirty: false,
            last_save: Instant::now(),
        }
    }
}
fn parse_sessions(items: &Value) -> Vec<Session> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .take(2000)
        .filter_map(|item| {
            let key = item["key"].as_str()?;
            Some(Session {
                key: key.into(),
                title: ["label", "derivedTitle", "displayName", "title"]
                    .iter()
                    .filter_map(|field| item[*field].as_str())
                    .find(|text| !text.trim().is_empty() && !text.starts_with("agent:"))
                    .unwrap_or("New chat")
                    .chars()
                    .take(240)
                    .collect(),
                running: item["hasActiveRun"] == true,
            })
        })
        .collect()
}
impl Catalogs {
    pub fn poll(
        &mut self,
        connections: &[preferences::Connection],
        active: &str,
        ctx: &egui::Context,
    ) {
        self.rows
            .retain(|url, _| connections.iter().any(|connection| &connection.url == url));
        for connection in connections {
            let row = self.rows.entry(connection.url.clone()).or_default();
            if connection.url == active {
                row.link = None;
                continue;
            }
            if row.link.is_none() {
                row.link = Some(gateway::connect_catalog(
                    connection.url.clone(),
                    ctx.clone(),
                ));
            }
            let events: Vec<_> = row
                .link
                .as_ref()
                .map(|link| link.rx.try_iter().take(32).collect())
                .unwrap_or_default();
            for event in events {
                match event {
                    gateway::Event::Status(status) => {
                        row.online = status == "Connected";
                        row.status = status;
                    }
                    gateway::Event::Error(error) => {
                        row.online = false;
                        row.status = error;
                    }
                    gateway::Event::Data(value) => {
                        if value["ok"] == true {
                            row.sessions = parse_sessions(&value["payload"]["sessions"]);
                            self.dirty = true;
                        } else {
                            row.status = value["error"]["message"]
                                .as_str()
                                .unwrap_or("Catalog unavailable")
                                .into();
                        }
                    }
                }
            }
        }
        if self.dirty && self.last_save.elapsed() >= Duration::from_secs(2) {
            self.save();
        }
    }
    pub fn update_active(
        &mut self,
        url: &str,
        sessions: &[(String, String)],
        running: &std::collections::HashSet<String>,
        online: bool,
        status: &str,
    ) {
        let row = self.rows.entry(url.into()).or_default();
        row.online = online;
        row.status = status.into();
        // Keep cached titles during reconnect rather than flash an empty group.
        if online || !sessions.is_empty() {
            let next: Vec<_> = sessions
                .iter()
                .take(2000)
                .map(|(key, title)| Session {
                    key: key.clone(),
                    title: title.clone(),
                    running: running.contains(key),
                })
                .collect();
            if row
                .sessions
                .iter()
                .map(|s| (&s.key, &s.title, s.running))
                .ne(next.iter().map(|s| (&s.key, &s.title, s.running)))
            {
                self.dirty = true;
            }
            row.sessions = next;
        }
    }
    pub fn select(&mut self, url: &str, key: &str) {
        self.rows.entry(url.into()).or_default().selected = Some(key.into());
        self.dirty = true;
    }
    pub fn toggle(&mut self, url: &str) {
        let row = self.rows.entry(url.into()).or_default();
        row.expanded = !row.expanded;
        self.dirty = true;
    }
    pub fn save(&mut self) {
        let value: Vec<_> = self.rows.iter().map(|(url,row)| json!({"url":url,"expanded":row.expanded,"selected":row.selected,
            "sessions":row.sessions.iter().map(|s|json!({"key":s.key,"title":s.title,"hasActiveRun":s.running})).collect::<Vec<_>>()})).collect();
        if preferences::write("server-catalogs.json", &json!(value)).is_ok() {
            self.dirty = false;
        }
        self.last_save = Instant::now();
    }
}
