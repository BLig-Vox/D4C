//! Each visited host retains its own authenticated chat transport across switches.
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
    warm: bool,
    media_token: Option<String>,
    refresh: Instant,
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
            warm: false,
            media_token: None,
            refresh: Instant::now(),
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
    pub fn park(
        &mut self,
        url: &str,
        link: gateway::Gateway,
        token: Option<String>,
        online: bool,
        status: &str,
    ) {
        let row = self.rows.entry(url.to_owned()).or_default();
        row.link = Some(link);
        row.warm = true;
        row.media_token = token;
        row.online = online;
        row.status = status.to_owned();
    }

    pub fn take_warm(
        &mut self,
        url: &str,
    ) -> Option<(gateway::Gateway, Option<String>, bool, String)> {
        let row = self.rows.get_mut(url)?;
        if !row.warm {
            return None;
        }
        row.warm = false;
        Some((
            row.link.take()?,
            row.media_token.take(),
            row.online,
            row.status.clone(),
        ))
    }

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
                row.warm = false;
                row.media_token = None;
                continue;
            }
            if row.link.is_none() {
                row.link = Some(gateway::connect_catalog(
                    connection.url.clone(),
                    ctx.clone(),
                ));
            }
            if row.warm && row.online && row.refresh.elapsed() >= Duration::from_secs(30) {
                if let Some(link) = &row.link {
                    let _ = link.tx.send(gateway::rpc(
                        "sessions.list",
                        json!({"includeDerivedTitles":true}),
                    ));
                }
                row.refresh = Instant::now();
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
                        if value["id"] == "connect" && value["ok"] == true {
                            row.media_token = value["payload"]["auth"]["deviceToken"]
                                .as_str()
                                .map(str::to_owned);
                        }
                        if !value["id"]
                            .as_str()
                            .is_some_and(|id| id.starts_with("sessions.list:"))
                        {
                            continue;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn switching_hosts_returns_the_same_transport_and_its_own_credentials() {
        let mut catalogs = Catalogs {
            rows: BTreeMap::new(),
            dirty: false,
            last_save: Instant::now(),
        };
        let mut commands = Vec::new();
        for host in ["first", "second"] {
            let (tx, rx) = mpsc::channel();
            let (_events, incoming) = mpsc::channel();
            catalogs.park(
                host,
                gateway::test_gateway(tx, incoming),
                Some(host.into()),
                true,
                "Connected",
            );
            commands.push(rx);
        }
        for (index, host) in ["first", "second"].into_iter().enumerate() {
            let (link, token, online, _) = catalogs.take_warm(host).unwrap();
            assert_eq!(token.as_deref(), Some(host));
            assert!(online);
            link.tx.send(json!(host)).unwrap();
            assert_eq!(commands[index].recv().unwrap(), json!(host));
            assert!(catalogs.take_warm(host).is_none());
        }
    }
}
