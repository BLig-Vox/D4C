use eframe::egui;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Default)]
pub struct Center {
    pub entries: Vec<Value>,
    seen: Vec<String>,
    owners: HashMap<String, (String, String)>,
    pub since: u64,
    file: String,
    pub error: String,
}
impl Center {
    pub fn load(server: &str) -> Self {
        let file = format!("activity-{:x}.json", Sha256::digest(server.as_bytes()));
        let v = crate::preferences::read(&file);
        Self {
            entries: v["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(100)
                .collect(),
            seen: v["seen"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .take(2000)
                .collect(),
            owners: v["owners"]
                .as_object()
                .into_iter()
                .flatten()
                .take(1000)
                .filter_map(|(k, v)| {
                    Some((k.clone(), (v[0].as_str()?.into(), v[1].as_str()?.into())))
                })
                .collect(),
            since: v["since"].as_u64().unwrap_or_else(now),
            file,
            error: String::new(),
        }
    }
    pub fn save(&mut self) {
        if self.file.is_empty() {
            return;
        }
        if let Err(e) = crate::preferences::write(
            &self.file,
            &json!({"entries":self.entries,"seen":self.seen,"owners":self.owners,"since":self.since}),
        ) {
            self.error = format!("Could not save notifications: {e}");
        }
    }
    pub fn push(&mut self, mut entry: Value) -> bool {
        let Some(id) = entry["id"].as_str().map(str::to_owned) else {
            return false;
        };
        if self.seen.contains(&id) {
            return false;
        }
        self.seen.push(id);
        if self.seen.len() > 2000 {
            self.seen.remove(0);
        }
        for key in ["body", "title"] {
            if let Some(s) = entry[key].as_str() {
                entry[key] = json!(
                    s.chars()
                        .take(if key == "title" { 120 } else { 16000 })
                        .collect::<String>()
                );
            }
        }
        self.entries.insert(0, entry);
        self.entries.truncate(100);
        self.save();
        true
    }
    pub fn unread(&self, session: Option<&str>) -> usize {
        self.entries
            .iter()
            .filter(|v| {
                v["unread"] == true && session.is_none_or(|s| v["session"].as_str() == Some(s))
            })
            .count()
    }
    pub fn read(&mut self, session: Option<&str>) {
        let mut changed = false;
        for v in &mut self.entries {
            if v["unread"] == true && session.is_none_or(|s| v["session"].as_str() == Some(s)) {
                v["unread"] = json!(false);
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }
    pub fn bind(&mut self, jobs: &[Value]) {
        let mut changed = false;
        for j in jobs {
            if let Some(id) = j["id"].as_str() {
                let name = j["name"].as_str().unwrap_or("Scheduled task");
                let session = j["owner"]["sessionKey"]
                    .as_str()
                    .or_else(|| j["sessionKey"].as_str())
                    .unwrap_or("");
                let owner = (name.to_string(), session.to_string());
                if self.owners.get(id) != Some(&owner) && self.owners.len() < 1000 {
                    self.owners.insert(id.into(), owner);
                    changed = true;
                }
            }
        }
        for entry in &mut self.entries {
            if let Some(job) = entry["job"].as_str() {
                if let Some((name, session)) = self.owners.get(job) {
                    if entry["session"].as_str() != Some(session)
                        || entry["title"].as_str() != Some(name)
                    {
                        entry["session"] = json!(session);
                        entry["title"] = json!(name);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.save();
        }
    }
    pub fn cron(&mut self, run: &Value) -> Option<Value> {
        if run["action"] != "finished" {
            return None;
        }
        if run["job"].is_object() {
            self.bind(&[run["job"].clone()]);
        }
        let job = run["jobId"].as_str()?;
        let ts = run["runAtMs"].as_u64().or_else(|| run["ts"].as_u64())?;
        let finished = run["ts"]
            .as_u64()
            .or_else(|| run["durationMs"].as_u64().map(|d| ts.saturating_add(d)))
            .unwrap_or_else(|| if run["_live"] == true { now() } else { ts });
        if finished < self.since || finished < now().saturating_sub(7 * 24 * 60 * 60 * 1000) {
            return None;
        }
        let runid = run["runId"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| ts.to_string());
        let (name, session) = self.owners.get(job).cloned().unwrap_or_else(|| {
            (
                run["jobName"].as_str().unwrap_or("Scheduled task").into(),
                String::new(),
            )
        });
        let failed = run["status"].as_str().is_some_and(|s| s != "ok");
        let body = run["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| run["error"].as_str().filter(|s| !s.is_empty()))
            .unwrap_or(if failed {
                "Task did not complete. Open history for details."
            } else {
                "Task finished. Open history for the result."
            });
        let needs_body = run["summary"].as_str().is_none_or(str::is_empty) && !failed;
        let entry = json!({"needsBody":needs_body,"id":format!("cron:{job}:{runid}"),"title":name,"body":body,"session":session,"job":job,"runId":run["runId"],"runAtMs":ts,"status":run["status"],"time":run["ts"].as_u64().unwrap_or(ts),"unread":true,"kind":"cron"});
        if let Some(existing) = self.entries.iter_mut().find(|v| v["id"] == entry["id"]) {
            if run["summary"].as_str().is_some_and(|s| !s.is_empty())
                && (existing["body"] != entry["body"] || existing["needsBody"] == true)
            {
                existing["body"] = entry["body"].clone();
                existing["needsBody"] = json!(false);
                self.save();
            }
            return None;
        }
        self.push(entry.clone()).then_some(entry)
    }
    pub fn pending_results(&self) -> Vec<Value> {
        self.entries.iter().filter(|v|v["kind"]=="cron" && (v["needsBody"]==true || v["body"].as_str().is_some_and(|s|s.starts_with("Task finished. Open history")))).map(|v|json!({"action":"finished","jobId":v["job"],"runId":v["runId"],"runAtMs":v["runAtMs"],"status":v["status"],"ts":v["time"]})).collect()
    }
}

pub struct Live {
    pub label: String,
    pub detail: String,
    pub started: Instant,
    pub steps: usize,
}
impl Default for Live {
    fn default() -> Self {
        Self {
            label: "Waiting for assistant".into(),
            detail: String::new(),
            started: Instant::now(),
            steps: 0,
        }
    }
}
impl Live {
    pub fn observe(&mut self, v: &Value) {
        let p = &v["payload"];
        let d = &p["data"];
        if v["event"] == "chat" && p["state"] == "delta" {
            self.label = "Replying".into();
        }
        if v["event"] != "agent" {
            return;
        }
        match p["stream"].as_str().unwrap_or("") {
            "reasoning" | "thinking" => {
                self.label = "Thinking".into();
                self.detail = "Reasoning summary appears below when available".into();
            }
            "tool" => {
                let name = d["name"].as_str().unwrap_or("tool");
                let phase = d["phase"].as_str().unwrap_or("");
                if matches!(phase, "start" | "update") {
                    self.label = if crate::cron::is_scheduler_tool(name) {
                        "Managing scheduled task".into()
                    } else {
                        format!("Using {}", name.replace('_', " "))
                    };
                    if phase == "start" {
                        self.steps += 1;
                    }
                } else {
                    self.label = "Working".into();
                }
                self.detail = if d["isError"] == true {
                    "Tool reported an error".into()
                } else {
                    String::new()
                };
            }
            "item" => {
                if d["kind"] == "answer_candidate" {
                    self.label = "Replying".into();
                    self.detail.clear();
                } else if d["kind"] == "preamble" {
                    self.label = "Working".into();
                    self.detail = d["progressText"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(140)
                        .collect();
                }
            }
            _ => {}
        }
    }
    pub fn ui(&self, ui: &mut egui::Ui) {
        ui.ctx().request_repaint_after(Duration::from_millis(80));
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(27, 35, 36))
            .corner_radius(12)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(28.0, 18.0), egui::Sense::hover());
                    let t = ui.input(|i| i.time);
                    for n in 0..3 {
                        let a = ((t * 4.0 - n as f64 * 0.8).sin() * 0.3 + 0.7) as f32;
                        ui.painter().circle_filled(
                            r.left_center() + egui::vec2(4.0 + n as f32 * 8.0, 0.0),
                            2.2,
                            egui::Color32::from_rgb(
                                (126.0 * a) as u8,
                                (211.0 * a) as u8,
                                (185.0 * a) as u8,
                            ),
                        );
                    }
                    ui.label(egui::RichText::new(&self.label).strong());
                    ui.label(
                        egui::RichText::new(format!("{}s", self.started.elapsed().as_secs()))
                            .small()
                            .weak(),
                    );
                    if self.steps > 0 {
                        ui.label(
                            egui::RichText::new(format!("· {} tools", self.steps))
                                .small()
                                .weak(),
                        );
                    }
                });
                if !self.detail.is_empty() {
                    ui.label(egui::RichText::new(&self.detail).small().weak());
                }
            });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_ownership_repairs_existing_notification() {
        let mut c = Center::default();
        let r = json!({"action":"finished","jobId":"late","runId":"run","runAtMs":now(),"summary":"Done"});
        assert!(c.cron(&r).is_some());
        assert_eq!(c.entries[0]["session"], "");
        c.bind(&[json!({"id":"late","name":"Late task","owner":{"sessionKey":"owner"}})]);
        assert_eq!(c.chat_results("owner").len(), 1);
        assert_eq!(c.entries[0]["title"], "Late task");
        assert!(c.cron(&r).is_none());
    }
    #[test]
    fn started_before_launch_but_finished_after_is_not_dropped() {
        let mut c = Center {
            since: now() - 1000,
            ..Default::default()
        };
        let r = json!({"action":"finished","jobId":"j","runAtMs":c.since-1000,"ts":now(),"status":"ok"});
        assert!(c.cron(&r).is_some());
    }
    #[test]
    fn cron_routes_to_owner_and_replays_once() {
        let mut c = Center::default();
        let job = json!({"id":"j","name":"Briefing","owner":{"sessionKey":"owner-chat"}});
        c.bind(&[job.clone()]);
        let run = json!({"action":"finished","jobId":"j","runId":"r","runAtMs":now(),"summary":"All done","status":"ok","sessionKey":"isolated-run-chat"});
        let item = c.cron(&run).unwrap();
        assert_eq!(item["session"], "owner-chat");
        assert!(c.cron(&run).is_none());
        assert_eq!(c.chat_results("owner-chat").len(), 1);
        assert!(c.chat_results("isolated-run-chat").is_empty());
        let mut newer = run.clone();
        newer["runId"] = json!("r2");
        newer["status"] = json!("error");
        newer["summary"] = json!("");
        newer["error"] = json!("Network unavailable");
        assert_eq!(c.cron(&newer).unwrap()["body"], "Network unavailable");
    }
    #[test]
    fn old_history_is_silent_and_event_snapshot_can_route_deleted_job() {
        let mut c = Center {
            since: now(),
            ..Default::default()
        };
        assert!(
            c.cron(&json!({"action":"finished","jobId":"j","runAtMs":c.since-1}))
                .is_none()
        );
        let r = json!({"action":"finished","jobId":"j","runAtMs":now(),"job":{"id":"j","name":"One shot","owner":{"sessionKey":"owner"}},"status":"ok"});
        assert_eq!(c.cron(&r).unwrap()["session"], "owner");
    }
    #[test]
    fn live_indicators_follow_real_streams() {
        let mut l = Live::default();
        l.observe(&json!({"event":"agent","payload":{"stream":"tool","data":{"name":"automations","phase":"start"}}}));
        assert_eq!(l.label, "Managing scheduled task");
        assert_eq!(l.steps, 1);
        l.observe(&json!({"event":"agent","payload":{"stream":"reasoning","data":{}}}));
        assert_eq!(l.label, "Thinking");
        l.observe(&json!({"event":"chat","payload":{"state":"delta"}}));
        assert_eq!(l.label, "Replying");
    }
    #[test]
    fn notification_deduplication_and_read_routing() {
        let mut c = Center::default();
        let entry =
            json!({"id":"task:run1","session":"chat-a","title":"Morning briefing","unread":true});
        assert!(c.push(entry.clone()));
        assert!(!c.push(entry));
        assert!(c.push(json!({"id":"task:run2","session":"chat-b","unread":true})));
        assert_eq!(c.unread(None), 2);
        c.read(Some("chat-a"));
        assert_eq!(c.unread(Some("chat-a")), 0);
        assert_eq!(c.unread(Some("chat-b")), 1);
    }
    #[test]
    fn bounded_notifications() {
        let mut c = Center::default();
        for n in 0..150 {
            c.push(json!({"id":n.to_string(),"unread":true}));
        }
        assert_eq!(c.entries.len(), 100);
    }
}
