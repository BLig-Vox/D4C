use crate::{gateway, icon_text};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Frequency {
    Daily,
    Weekly,
    Every,
    Once,
    Cron,
}
impl Frequency {
    fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Every => "Every interval",
            Self::Once => "Once",
            Self::Cron => "Cron expression",
        }
    }
}
#[derive(Clone)]
pub struct Editor {
    original: Option<Value>,
    pub name: String,
    pub prompt: String,
    pub frequency: Frequency,
    pub time: String,
    pub timezone: String,
    pub weekday: usize,
    pub interval: String,
    pub unit: u64,
    pub at: String,
    pub expression: String,
    pub enabled: bool,
    pub model: String,
    pub thinking: String,
    original_schedule_inputs: String,
}
impl Editor {
    pub fn new(model: &str, thinking: &str) -> Self {
        Self {
            original: None,
            name: String::new(),
            prompt: String::new(),
            frequency: Frequency::Daily,
            time: "09:00".into(),
            timezone: std::env::var("TZ").unwrap_or_else(|_| "UTC".into()),
            weekday: 0,
            interval: "1".into(),
            unit: 3_600_000,
            at: String::new(),
            expression: "0 9 * * *".into(),
            enabled: true,
            model: model.into(),
            thinking: thinking.into(),
            original_schedule_inputs: String::new(),
        }
    }
    #[cfg(test)]
    fn from_job(job: &Value) -> Option<Self> {
        if !matches!(
            job["payload"]["kind"].as_str(),
            Some("agentTurn" | "systemEvent")
        ) {
            return None;
        }
        let mut form = Self::new(
            job["payload"]["model"].as_str().unwrap_or(""),
            job["payload"]["thinking"].as_str().unwrap_or(""),
        );
        form.name = job["name"].as_str().unwrap_or("").into();
        form.prompt = job["payload"]["message"]
            .as_str()
            .or_else(|| job["payload"]["text"].as_str())
            .unwrap_or("")
            .into();
        form.enabled = job["enabled"].as_bool().unwrap_or(false);
        match job["schedule"]["kind"].as_str()? {
            "every" => {
                form.frequency = Frequency::Every;
                let ms = job["schedule"]["everyMs"].as_u64()?;
                form.unit = [86_400_000, 3_600_000, 60_000, 1000, 1]
                    .into_iter()
                    .find(|unit| ms % unit == 0)
                    .unwrap_or(1);
                form.interval = (ms / form.unit).to_string();
            }
            "at" => {
                form.frequency = Frequency::Once;
                form.at = job["schedule"]["at"].as_str()?.into();
            }
            "cron" => {
                form.frequency = Frequency::Cron;
                form.expression = job["schedule"]["expr"].as_str()?.into();
                form.timezone = job["schedule"]["tz"].as_str().unwrap_or("").into();
            }
            _ => return None,
        }
        form.original_schedule_inputs = form.schedule_inputs();
        form.original = Some(job.clone());
        Some(form)
    }
    fn schedule_inputs(&self) -> String {
        format!(
            "{:?}|{}|{}|{}|{}|{}|{}|{}",
            self.frequency,
            self.time,
            self.timezone,
            self.weekday,
            self.interval,
            self.unit,
            self.at,
            self.expression
        )
    }
    pub fn schedule(&self) -> Result<Value, String> {
        if self.original.is_some() && self.schedule_inputs() == self.original_schedule_inputs {
            return Ok(self.original.as_ref().unwrap()["schedule"].clone());
        }
        match self.frequency {
            Frequency::Every => {
                let amount = self
                    .interval
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "Enter a positive interval")?;
                let ms = amount
                    .checked_mul(self.unit)
                    .filter(|v| *v > 0 && *v <= 8_640_000_000_000_000)
                    .ok_or("Interval is zero or too large")?;
                Ok(json!({"kind":"every","everyMs":ms}))
            }
            Frequency::Once => {
                let at = self.at.trim();
                let (_, clock) = at
                    .split_once('T')
                    .ok_or("Use an ISO date with timezone, e.g. 2026-11-18T09:00:00+08:00")?;
                if !clock.ends_with('Z') && !clock.contains('+') && !clock.contains('-') {
                    return Err("Include a timezone offset or Z in the date".into());
                }
                Ok(json!({"kind":"at","at":at}))
            }
            kind => {
                let expr = if kind == Frequency::Cron {
                    let expr = self.expression.trim();
                    if !(5..=6).contains(&expr.split_whitespace().count()) {
                        return Err("Use a 5 or 6 field cron expression".into());
                    }
                    expr.to_owned()
                } else {
                    let (hour, minute) = self
                        .time
                        .trim()
                        .split_once(':')
                        .ok_or("Enter time as HH:MM")?;
                    let hour = hour
                        .parse::<u8>()
                        .ok()
                        .filter(|v| *v < 24)
                        .ok_or("Hour must be 00–23")?;
                    let minute = minute
                        .parse::<u8>()
                        .ok()
                        .filter(|v| *v < 60)
                        .ok_or("Minute must be 00–59")?;
                    if kind == Frequency::Weekly {
                        format!("{minute} {hour} * * {}", (self.weekday + 1) % 7)
                    } else {
                        format!("{minute} {hour} * * *")
                    }
                };
                let mut schedule = json!({"kind":"cron","expr":expr});
                if !self.timezone.trim().is_empty() {
                    schedule["tz"] = json!(self.timezone.trim());
                }
                Ok(schedule)
            }
        }
    }
    pub fn request(&self) -> Result<(&'static str, Value), String> {
        if self.name.trim().is_empty() || self.prompt.trim().is_empty() {
            return Err("Give the task a name and instructions".into());
        }
        if self.name.len() > 200 || self.prompt.len() > 32_000 {
            return Err("Use a name under 200 bytes and instructions under 32 KB".into());
        }
        let schedule = self.schedule()?;
        if let Some(original) = &self.original {
            let kind = original["payload"]["kind"].as_str().unwrap_or("agentTurn");
            let mut payload = json!({"kind":kind});
            payload[if kind == "systemEvent" {
                "text"
            } else {
                "message"
            }] = json!(self.prompt.trim());
            if kind == "agentTurn" {
                if self.model != original["payload"]["model"].as_str().unwrap_or("")
                    && !self.model.is_empty()
                {
                    payload["model"] = json!(self.model);
                }
                if self.thinking != original["payload"]["thinking"].as_str().unwrap_or("")
                    && !self.thinking.is_empty()
                {
                    payload["thinking"] = json!(self.thinking);
                }
            }
            let mut params = json!({"id":original["id"],"patch":{"name":self.name.trim(),"enabled":self.enabled,"schedule":schedule,"payload":payload}});
            if let Some(revision) = original["configRevision"].as_str() {
                params["expectedConfigRevision"] = json!(revision);
            }
            Ok(("cron.update", params))
        } else {
            let mut payload =
                json!({"kind":"agentTurn","message":self.prompt.trim(),"timeoutSeconds":120});
            if !self.model.is_empty() {
                payload["model"] = json!(self.model);
            }
            if !self.thinking.is_empty() {
                payload["thinking"] = json!(self.thinking);
            }
            Ok((
                "cron.add",
                json!({"name":self.name.trim(),"enabled":self.enabled,"deleteAfterRun":false,"schedule":schedule,"sessionTarget":"isolated","wakeMode":"now","payload":payload,"delivery":{"mode":"none"}}),
            ))
        }
    }
}

#[derive(Clone)]
enum Pending {
    List(usize),
    Status,
    Feed(usize),
    ResultBody(Value),
    Save,
    Toggle,
    Delete,
    Run(String),
    Runs(String, usize),
    Transcript(String),
}
struct Request {
    kind: Pending,
    started: Instant,
}
pub struct ChatRequest {
    pub session: Option<String>,
    pub draft: String,
}
#[derive(Default)]
pub struct Tasks {
    jobs: Vec<Value>,
    pending: HashMap<String, Request>,
    outbox: Vec<(String, Value, Pending)>,
    editor: Option<Editor>,
    delete: Option<Value>,
    manage: Option<String>,
    history: Option<(String, String)>,
    runs: Vec<Value>,
    transcript: Option<(String, Vec<Value>)>,
    scheduler: Value,
    error: String,
    notice: String,
    form_error: String,
    last_refresh: Option<Instant>,
    dirty: bool,
    has_more: bool,
    runs_more: bool,
    filter: String,
    scope_session: Option<String>,
    chat_request: Option<ChatRequest>,
    chat_links: HashMap<String, Vec<String>>,
    pub open_history: bool,
    finished: Vec<Value>,
    feed_checked: Option<Instant>,
    hydrated: HashSet<String>,
}
fn belongs_to_session(job: &Value, session: &str, linked: &[String]) -> bool {
    if let Some(owner) = job["owner"]["sessionKey"]
        .as_str()
        .or_else(|| job["sessionKey"].as_str())
    {
        owner == session
    } else {
        job["id"]
            .as_str()
            .is_some_and(|id| linked.iter().any(|key| key == id))
    }
}
impl Tasks {
    pub fn jobs(&self) -> &[Value] {
        &self.jobs
    }
    pub fn load_result(&mut self, run: &Value) {
        if self
            .pending
            .values()
            .filter(|r| matches!(r.kind, Pending::ResultBody(_)))
            .count()
            + self
                .outbox
                .iter()
                .filter(|(_, _, kind)| matches!(kind, Pending::ResultBody(_)))
                .count()
            >= 8
        {
            return;
        }
        let Some(id) = run["jobId"].as_str() else {
            return;
        };
        let mut params = json!({"id":id,"limit":30});
        if let Some(runid) = run["runId"].as_str() {
            params["runId"] = json!(runid);
        } else if let Some(ts) = run["runAtMs"].as_u64() {
            params["runAtMs"] = json!(ts);
        } else {
            return;
        }
        let key = format!(
            "{}:{}",
            id,
            params["runId"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| params["runAtMs"].to_string())
        );
        if self.hydrated.contains(&key) || self.hydrated.len() >= 1000 {
            return;
        }
        self.hydrated.insert(key);
        self.queue("cron.history", params, Pending::ResultBody(run.clone()));
    }
    pub fn take_finished(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.finished)
    }
    pub fn background(&mut self, connected: bool) {
        if !connected {
            return;
        }
        if self.dirty
            || self
                .last_refresh
                .is_none_or(|t| t.elapsed() > Duration::from_secs(30))
        {
            self.refresh();
        }
        if self
            .feed_checked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(20))
            && !self
                .pending
                .values()
                .any(|r| matches!(r.kind, Pending::Feed(_)))
            && !self
                .outbox
                .iter()
                .any(|(_, _, k)| matches!(k, Pending::Feed(_)))
        {
            self.feed_checked = Some(Instant::now());
            self.queue(
                "cron.runs",
                json!({"scope":"all","limit":100,"sortDir":"desc","offset":0}),
                Pending::Feed(0),
            );
        }
    }
    pub fn show_history(&mut self, id: &str, name: &str) {
        self.history = Some((id.into(), name.into()));
        self.runs.clear();
        self.transcript = None;
        self.load_runs(id, 0);
        self.open_history = true;
    }
    pub fn take_chat_request(&mut self) -> Option<ChatRequest> {
        self.chat_request.take()
    }
    pub fn observe_tool(&mut self, session: &str, result: &Value) {
        for job in task_results(result) {
            let Some(id) = job["id"].as_str().map(str::to_owned) else {
                continue;
            };
            let links = self.chat_links.entry(session.into()).or_default();
            if !links.contains(&id) {
                links.push(id.clone());
            }
            if let Some(existing) = self.jobs.iter_mut().find(|j| j["id"] == id) {
                *existing = job;
            } else {
                self.jobs.push(job);
            }
        }
        self.dirty = true;
    }
    fn edit_in_chat(&mut self, job: &Value) {
        self.chat_request = Some(ChatRequest {
            session: job["owner"]["sessionKey"]
                .as_str()
                .or_else(|| job["sessionKey"].as_str())
                .map(str::to_owned),
            draft: format!(
                "Change my scheduled task \"{}\": ",
                job["name"].as_str().unwrap_or("Task")
            ),
        });
    }
    pub fn chat_ui(&mut self, ui: &mut egui::Ui, session: &str, connected: bool) {
        let linked = self.chat_links.get(session).cloned().unwrap_or_default();
        let jobs: Vec<_> = self
            .jobs
            .iter()
            .filter(|j| belongs_to_session(j, session, &linked))
            .cloned()
            .collect();
        if connected
            && (self.dirty
                || ((!linked.is_empty() || !jobs.is_empty())
                    && self
                        .last_refresh
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(15))))
        {
            self.refresh();
        }
        for job in jobs {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(29, 44, 42))
                .corner_radius(12)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.set_width((ui.available_width() * 0.90).min(732.0).max(1.0));
                    ui.horizontal(|ui| {
                        ui.label(icon_text(lucide_icons::Icon::CalendarClock, 17.0));
                        ui.label(
                            egui::RichText::new("Scheduled task")
                                .small()
                                .color(egui::Color32::from_rgb(164, 206, 190)),
                        );
                    });
                    ui.strong(job["name"].as_str().unwrap_or("Task"));
                    ui.label(schedule_label(&job["schedule"]));
                    let state = if job["enabled"] == true {
                        "Active"
                    } else {
                        "Paused"
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{state}{}",
                            job["state"]["nextRunAtMs"]
                                .as_u64()
                                .map(|t| format!(" · Next: {}", timestamp(t)))
                                .unwrap_or_default()
                        ))
                        .small()
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Change in chat").clicked() {
                            self.edit_in_chat(&job);
                        }
                        if ui.button("View history").clicked() {
                            if let Some(id) = job["id"].as_str() {
                                self.history = Some((
                                    id.into(),
                                    job["name"].as_str().unwrap_or("Task").into(),
                                ));
                                self.runs.clear();
                                self.load_runs(id, 0);
                                self.open_history = true;
                            }
                        }
                    });
                });
        }
    }

    fn queue(&mut self, method: &str, params: Value, kind: Pending) {
        self.outbox.push((method.into(), params, kind));
    }
    fn is_listing(&self) -> bool {
        self.pending
            .values()
            .any(|r| matches!(r.kind, Pending::List(_)))
            || self
                .outbox
                .iter()
                .any(|(_, _, k)| matches!(k, Pending::List(_)))
    }
    fn mutating(&self) -> bool {
        self.pending.values().any(|r| {
            matches!(
                r.kind,
                Pending::Save | Pending::Toggle | Pending::Delete | Pending::Run(_)
            )
        }) || self.outbox.iter().any(|(_, _, k)| {
            matches!(
                k,
                Pending::Save | Pending::Toggle | Pending::Delete | Pending::Run(_)
            )
        })
    }
    pub fn refresh(&mut self) {
        if self.is_listing() {
            return;
        }
        self.dirty = false;
        self.last_refresh = Some(Instant::now());
        self.queue(
            "cron.list",
            json!({"includeDisabled":true,"limit":100,"offset":0}),
            Pending::List(0),
        );
        if !self
            .pending
            .values()
            .any(|r| matches!(r.kind, Pending::Status))
        {
            self.queue("cron.status", json!({}), Pending::Status);
        }
    }
    fn load_runs(&mut self, id: &str, offset: usize) {
        if self
            .pending
            .values()
            .any(|r| matches!(&r.kind,Pending::Runs(key,_) if key==id))
        {
            return;
        }
        self.queue(
            "cron.runs",
            json!({"id":id,"limit":30,"offset":offset}),
            Pending::Runs(id.into(), offset),
        );
    }
    pub fn flush(&mut self, gateway: Option<&gateway::Gateway>) {
        for (method, params, kind) in std::mem::take(&mut self.outbox) {
            let request = gateway::rpc(&method, params);
            let id = request["id"].as_str().unwrap().to_string();
            if gateway.is_some_and(|g| g.tx.send(request).is_ok()) {
                self.pending.insert(
                    id,
                    Request {
                        kind,
                        started: Instant::now(),
                    },
                );
            } else {
                self.error = "Connect to the server to manage tasks".into();
            }
        }
    }
    pub fn disconnected(&mut self) {
        if self.mutating() {
            self.error="Connection lost. The operation's outcome is unknown; refresh tasks before retrying.".into();
        }
        self.pending.clear();
        self.outbox.clear();
        self.last_refresh = None;
        self.feed_checked = None;
        self.hydrated.clear();
    }
    pub fn handle(&mut self, v: &Value) -> bool {
        if v["event"] == "cron" {
            self.dirty = true;
            let p = &v["payload"];
            if p["action"] == "finished" {
                let mut run = p.clone();
                run["_live"] = json!(true);
                self.finished.push(run);
                self.feed_checked = None;
            }
            return true;
        }
        let Some(request) = v["id"].as_str().and_then(|id| self.pending.remove(id)) else {
            return false;
        };
        if v["ok"] != true {
            let message = v["error"]["message"]
                .as_str()
                .unwrap_or("Scheduler request failed")
                .to_owned();
            if matches!(request.kind, Pending::Save) {
                self.form_error = message.clone();
            }
            self.error = message;
            return true;
        }
        let p = &v["payload"];
        match request.kind {
            Pending::List(offset) => {
                if let Some(jobs) = p["jobs"].as_array() {
                    if offset == 0 {
                        self.jobs = jobs.clone();
                    } else {
                        for job in jobs {
                            if !self.jobs.iter().any(|j| j["id"] == job["id"]) {
                                self.jobs.push(job.clone());
                            }
                        }
                    }
                    self.has_more = p["hasMore"].as_bool().unwrap_or(false);
                } else {
                    self.error = "Gateway returned an unexpected task list".into();
                }
            }
            Pending::ResultBody(mut run) => {
                let summary = p["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|m| m["role"] == "assistant")
                    .map(|m| {
                        m["content"].as_str().map(str::to_owned).unwrap_or_else(|| {
                            m["content"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                    })
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !summary.is_empty() {
                    run["summary"] = json!(summary.chars().take(16000).collect::<String>());
                    self.finished.push(run);
                }
            }
            Pending::Feed(offset) => {
                if let Some(entries) = p["entries"].as_array() {
                    self.finished.extend(entries.iter().cloned());
                    // Bounded catch-up: at most 300 recent runs per polling cycle.
                    if p["hasMore"] == true && offset < 200 {
                        self.queue(
                            "cron.runs",
                            json!({"scope":"all","limit":100,"sortDir":"desc","offset":offset+100}),
                            Pending::Feed(offset + 100),
                        );
                    }
                } else {
                    self.error = "Unexpected scheduler activity response".into();
                }
            }
            Pending::Status => self.scheduler = p.clone(),
            Pending::Save => {
                self.editor = None;
                self.form_error.clear();
                self.error.clear();
                self.notice = "Task saved on the server".into();
                self.dirty = true;
            }
            Pending::Toggle => {
                self.error.clear();
                self.notice = "Task updated".into();
                self.dirty = true;
            }
            Pending::Delete => {
                if p["removed"] == false {
                    self.error = "Task was already removed; refresh the list".into();
                } else {
                    self.notice = "Task deleted".into();
                    self.error.clear();
                }
                self.delete = None;
                self.dirty = true;
            }
            Pending::Run(id) => {
                self.notice = if p["ran"] == false {
                    format!(
                        "Run not started: {}",
                        p["reason"].as_str().unwrap_or("not admitted")
                    )
                } else {
                    "Run requested. History will show the result when it finishes.".into()
                };
                self.dirty = true;
                if self.history.as_ref().is_some_and(|(key, _)| key == &id) {
                    self.load_runs(&id, 0);
                }
            }
            Pending::Runs(id, offset) => {
                if self.history.as_ref().is_some_and(|(key, _)| key == &id) {
                    if let Some(entries) = p["entries"].as_array() {
                        if offset == 0 {
                            self.runs = entries.clone();
                        } else {
                            self.runs.extend(entries.iter().cloned());
                        }
                        self.runs_more = p["hasMore"].as_bool().unwrap_or(false);
                    } else {
                        self.error = "Gateway returned an unexpected run history".into();
                    }
                }
            }
            Pending::Transcript(key) => {
                if self.transcript.as_ref().is_some_and(|(id, _)| id == &key) {
                    self.transcript =
                        Some((key, p["messages"].as_array().cloned().unwrap_or_default()));
                }
            }
        }
        true
    }
    pub fn session_ui(
        &mut self,
        ui: &mut egui::Ui,
        session: &str,
        connected: bool,
        models: &[String],
        model: &str,
        thinking: &str,
    ) {
        let all = std::mem::take(&mut self.jobs);
        let linked = self.chat_links.get(session).cloned().unwrap_or_default();
        self.jobs = all
            .iter()
            .filter(|j| belongs_to_session(j, session, &linked))
            .cloned()
            .collect();
        if self
            .history
            .as_ref()
            .is_some_and(|(id, _)| !self.jobs.iter().any(|j| j["id"].as_str() == Some(id)))
        {
            self.history = None;
            self.runs.clear();
            self.transcript = None;
        }
        if self
            .manage
            .as_ref()
            .is_some_and(|id| !self.jobs.iter().any(|j| j["id"].as_str() == Some(id)))
        {
            self.manage = None;
        }
        self.scope_session = Some(session.into());
        let more = self.has_more;
        self.has_more = false;
        self.ui(ui, connected, models, model, thinking);
        self.jobs = all;
        self.has_more = more;
        self.scope_session = None;
        if let Some(request) = self.chat_request.as_mut() {
            if request.session.is_none() {
                request.session = Some(session.into());
            }
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        connected: bool,
        models: &[String],
        model: &str,
        thinking: &str,
    ) {
        if connected
            && (self.last_refresh.is_none()
                || (self.dirty && !self.is_listing())
                || self
                    .last_refresh
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(15)))
        {
            self.refresh();
            if let Some((id, _)) = self.history.clone() {
                self.load_runs(&id, 0);
            }
        }
        ui.ctx().request_repaint_after(Duration::from_secs(15));
        if self
            .pending
            .values()
            .any(|r| r.started.elapsed() > Duration::from_secs(60))
        {
            ui.label(
                egui::RichText::new(
                    "The server is taking longer than expected. Waiting for confirmation…",
                )
                .weak(),
            );
        }

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    connected && !self.mutating(),
                    egui::Button::new(crate::button_label(
                        lucide_icons::Icon::Plus,
                        "Schedule in chat",
                    ))
                    .min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                self.chat_request = Some(ChatRequest {
                    session: None,
                    draft: String::new(),
                });
            }
            if ui
                .add_enabled(
                    connected && !self.is_listing(),
                    egui::Button::new("Refresh").min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                self.error.clear();
                self.refresh();
                if let Some((id, _)) = self.history.clone() {
                    self.load_runs(&id, 0);
                }
            }
        });
        ui.add_space(4.0);
        if self.scope_session.is_none() && ui.small_button("Advanced").clicked() {
            self.editor = Some(Editor::new(model, thinking));
            self.form_error.clear();
        }
        let scheduler = if !connected {
            "Connect to the server to manage scheduled work"
        } else if self.scheduler["enabled"] == false {
            "The server scheduler is disabled. Tasks will not run on schedule."
        } else {
            "Runs on the server, even while D4C is closed · Times shown in UTC"
        };
        ui.label(egui::RichText::new(scheduler).small().weak());
        ui.add_space(12.0);
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
        }
        if !self.notice.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&self.notice).color(egui::Color32::from_rgb(161, 204, 187)),
                );
                if ui.small_button("Dismiss").clicked() {
                    self.notice.clear();
                }
            });
        }
        if let Some((id, name)) = self.history.clone() {
            self.history_ui(ui, &id, &name, connected);
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Search tasks…")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(12.0);
            if self.is_listing() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading tasks…");
                });
            }
            let filter = self.filter.to_lowercase();
            egui::ScrollArea::vertical()
                .id_salt("cron_jobs")
                .show(ui, |ui| {
                    if self.jobs.is_empty() && !self.is_listing() {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                icon_text(lucide_icons::Icon::CalendarClock, 30.0)
                                    .color(egui::Color32::from_gray(145)),
                            );
                            ui.add_space(10.0);
                            ui.heading("Make room for later");
                            ui.label("Create a task and let the server handle it on schedule.");
                        });
                    }
                    for job in self.jobs.clone().into_iter().filter(|j| {
                        j["name"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&filter)
                    }) {
                        let id = job["id"].as_str().unwrap_or("").to_string();
                        let name = job["name"].as_str().unwrap_or("Untitled task").to_string();
                        let enabled = job["enabled"].as_bool().unwrap_or(false);
                        let running = job["state"]["runningAtMs"].is_number();
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(31, 33, 38))
                            .corner_radius(12)
                            .inner_margin(16)
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        icon_text(lucide_icons::Icon::CalendarClock, 18.0)
                                            .color(egui::Color32::from_gray(180)),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&name).size(15.0).strong(),
                                        )
                                        .truncate(),
                                    );
                                    ui.label(
                                        egui::RichText::new(if running {
                                            "Running"
                                        } else if enabled {
                                            "Active"
                                        } else {
                                            "Paused"
                                        })
                                        .small()
                                        .color(
                                            if enabled {
                                                egui::Color32::from_rgb(157, 203, 180)
                                            } else {
                                                egui::Color32::from_gray(150)
                                            },
                                        ),
                                    );
                                });
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(schedule_label(&job["schedule"])).small(),
                                );
                                let prompt = job["payload"]["message"]
                                    .as_str()
                                    .or_else(|| job["payload"]["text"].as_str())
                                    .unwrap_or("Managed by gateway");
                                ui.add(
                                    egui::Label::new(egui::RichText::new(prompt).weak()).truncate(),
                                )
                                .on_hover_text(prompt);
                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    let next = if enabled {
                                        job["state"]["nextRunAtMs"]
                                            .as_u64()
                                            .map(timestamp)
                                            .unwrap_or_else(|| "Awaiting scheduler".into())
                                    } else {
                                        "Paused".into()
                                    };
                                    ui.label(
                                        egui::RichText::new(format!("Next: {next}")).small().weak(),
                                    );
                                    if let Some(last) = job["state"]["lastRunAtMs"].as_u64() {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Last: {} · {}",
                                                timestamp(last),
                                                job["state"]["lastRunStatus"]
                                                    .as_str()
                                                    .or_else(|| job["state"]["lastStatus"].as_str())
                                                    .unwrap_or("unknown")
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    }
                                });
                                if let Some(error) = job["state"]["lastError"].as_str() {
                                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                                }
                                ui.add_space(8.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().interact_size.y = 36.0;
                                    let free = connected && !self.mutating();
                                    if ui
                                        .add_enabled(
                                            connected,
                                            egui::Button::new(crate::button_label(
                                                lucide_icons::Icon::History,
                                                "History",
                                            ))
                                            .min_size(egui::vec2(80.0, 36.0)),
                                        )
                                        .clicked()
                                    {
                                        self.show_history(&id, &name);
                                    }
                                    if ui
                                        .selectable_label(
                                            self.manage.as_deref() == Some(id.as_str()),
                                            "Manage",
                                        )
                                        .clicked()
                                    {
                                        self.manage = if self.manage.as_deref() == Some(id.as_str())
                                        {
                                            None
                                        } else {
                                            Some(id.clone())
                                        };
                                    }
                                });
                                if self.manage.as_deref() == Some(id.as_str()) {
                                    ui.separator();
                                    ui.horizontal_wrapped(|ui| {
                                        let free = connected && !self.mutating();
                                        if ui
                                            .add_enabled(
                                                free && !running,
                                                egui::Button::new("Run now"),
                                            )
                                            .clicked()
                                        {
                                            self.queue(
                                                "cron.run",
                                                json!({"id":id,"mode":"force"}),
                                                Pending::Run(id.clone()),
                                            );
                                            self.manage = None;
                                        }
                                        if ui
                                            .add_enabled(
                                                free,
                                                egui::Button::new(if enabled {
                                                    "Pause schedule"
                                                } else {
                                                    "Resume schedule"
                                                }),
                                            )
                                            .clicked()
                                        {
                                            let mut params =
                                                json!({"id":id,"patch":{"enabled":!enabled}});
                                            if let Some(rev) = job["configRevision"].as_str() {
                                                params["expectedConfigRevision"] = json!(rev);
                                            }
                                            self.queue("cron.update", params, Pending::Toggle);
                                            self.manage = None;
                                        }
                                        if ui
                                            .add_enabled(free, egui::Button::new("Change in chat"))
                                            .clicked()
                                        {
                                            self.edit_in_chat(&job);
                                            self.manage = None;
                                        }
                                        ui.separator();
                                        if ui
                                            .add_enabled(free, egui::Button::new("Delete task"))
                                            .clicked()
                                        {
                                            self.delete = Some(job.clone());
                                            self.manage = None;
                                        }
                                    });
                                }
                            });
                        ui.add_space(10.0);
                    }
                    if self.has_more
                        && ui
                            .add_enabled(!self.is_listing(), egui::Button::new("Load more tasks"))
                            .clicked()
                    {
                        let offset = self.jobs.len();
                        self.queue(
                            "cron.list",
                            json!({"includeDisabled":true,"limit":100,"offset":offset}),
                            Pending::List(offset),
                        );
                    }
                });
        }
        self.editor_ui(ui, connected, models);
        if let Some(job) = self.delete.clone() {
            egui::Window::new("Delete scheduled task?")
                .max_width((ui.ctx().content_rect().width() - 48.0).max(180.0))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(job["name"].as_str().unwrap_or("This task"));
                    ui.label("This removes its future schedule. A running task may still finish.");
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.mutating(), egui::Button::new("Cancel"))
                            .clicked()
                        {
                            self.delete = None;
                        }
                        if ui
                            .add_enabled(
                                connected && !self.mutating(),
                                egui::Button::new("Delete task"),
                            )
                            .clicked()
                        {
                            self.queue("cron.remove", json!({"id":job["id"]}), Pending::Delete);
                        }
                    });
                });
        }
    }
    fn history_ui(&mut self, ui: &mut egui::Ui, id: &str, name: &str, connected: bool) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("← All tasks").clicked() {
                self.history = None;
                self.transcript = None;
            }
            ui.strong(name);
        });
        ui.add_space(10.0);
        if self
            .pending
            .values()
            .any(|r| matches!(&r.kind,Pending::Runs(key,_) if key==id))
        {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading run history…");
            });
        }
        egui::ScrollArea::vertical()
            .id_salt("cron_runs")
            .show(ui, |ui| {
                if self.runs.is_empty() {
                    ui.label("No completed runs yet. Run now or wait for the next scheduled time.");
                }
                for (index, run) in self.runs.clone().iter().enumerate() {
                    ui.push_id(index, |ui| {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(31, 33, 38))
                            .corner_radius(10)
                            .inner_margin(14)
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    let status = run["status"].as_str().unwrap_or("unknown");
                                    ui.label(egui::RichText::new(status).strong().color(
                                        if status == "ok" {
                                            egui::Color32::from_rgb(157, 203, 180)
                                        } else {
                                            egui::Color32::LIGHT_RED
                                        },
                                    ));
                                    ui.label(
                                        run["runAtMs"]
                                            .as_u64()
                                            .or_else(|| run["ts"].as_u64())
                                            .map(timestamp)
                                            .unwrap_or_default(),
                                    );
                                    if let Some(ms) = run["durationMs"].as_u64() {
                                        ui.label(format!("{:.1}s", ms as f64 / 1000.0));
                                    }
                                });
                                if let Some(summary) = run["summary"].as_str() {
                                    ui.add(egui::Label::new(summary).wrap().selectable(true));
                                }
                                if let Some(error) = run["error"].as_str() {
                                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                                }
                                if let Some(run_id) = run["runId"].as_str() {
                                    if ui
                                        .add_enabled(
                                            connected,
                                            egui::Button::new("View run messages"),
                                        )
                                        .clicked()
                                    {
                                        self.transcript = Some((run_id.into(), vec![]));
                                        self.queue(
                                            "cron.history",
                                            json!({"id":id,"runId":run_id,"limit":100}),
                                            Pending::Transcript(run_id.into()),
                                        );
                                    }
                                    if let Some((key, messages)) = &self.transcript {
                                        if key == run_id {
                                            for message in messages {
                                                ui.strong(
                                                    message["role"].as_str().unwrap_or("Message"),
                                                );
                                                ui.add(
                                                    egui::Label::new(crate::message_text(message))
                                                        .wrap()
                                                        .selectable(true),
                                                );
                                            }
                                        }
                                    }
                                }
                            });
                        ui.add_space(8.0);
                    });
                }
                if self.runs_more && ui.button("Older runs").clicked() {
                    self.load_runs(id, self.runs.len());
                }
            });
    }
    fn editor_ui(&mut self, ui: &mut egui::Ui, connected: bool, models: &[String]) {
        let Some(mut editor) = self.editor.take() else {
            return;
        };
        let mut cancel = false;
        let mut submit = false;
        let busy = self.mutating();
        let mut open = true;
        egui::Window::new(if editor.original.is_some(){"Edit task"}else{"New task"}).id(egui::Id::unique("cron_editor")).open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER,[0.0,0.0]).collapsible(false).resizable(false).default_width(470.0_f32.min((ui.ctx().content_rect().width()-48.0).max(180.0))).max_width((ui.ctx().content_rect().width()-48.0).max(180.0)).show(ui.ctx(),|ui|{
                ui.spacing_mut().interact_size.y=32.0;
                egui::ScrollArea::vertical().id_salt("task_form").max_height(520.0).show(ui,|ui|{
                    ui.label("Name");ui.add(egui::TextEdit::singleline(&mut editor.name).hint_text("Morning project summary").desired_width(f32::INFINITY));
                    ui.add_space(8.0);ui.label("Instructions");ui.add(egui::TextEdit::multiline(&mut editor.prompt).hint_text("What should the server do?").desired_rows(4).desired_width(f32::INFINITY));
                    ui.add_space(8.0);ui.horizontal_wrapped(|ui|{ui.label("Schedule");egui::ComboBox::from_id_salt("frequency").selected_text(editor.frequency.label()).show_ui(ui,|ui|{for frequency in [Frequency::Daily,Frequency::Weekly,Frequency::Every,Frequency::Once,Frequency::Cron]{ui.selectable_value(&mut editor.frequency,frequency,frequency.label());}});});
                    match editor.frequency{
                        Frequency::Every=>{ui.horizontal_wrapped(|ui|{ui.label("Every");ui.add(egui::TextEdit::singleline(&mut editor.interval).desired_width(65.0));let unit_label=match editor.unit{1=>"milliseconds",1000=>"seconds",60_000=>"minutes",3_600_000=>"hours",_=>"days"};egui::ComboBox::from_id_salt("interval_unit").selected_text(unit_label).show_ui(ui,|ui|{for (ms,label) in [(1000,"seconds"),(60_000,"minutes"),(3_600_000,"hours"),(86_400_000,"days")]{ui.selectable_value(&mut editor.unit,ms,label);}});});}
                        Frequency::Once=>{ui.label("Date and time, including timezone");ui.add(egui::TextEdit::singleline(&mut editor.at).hint_text("2026-11-18T09:00:00+08:00").desired_width(f32::INFINITY));}
                        kind=>{
                            if kind==Frequency::Cron{ui.add(egui::TextEdit::singleline(&mut editor.expression).hint_text("0 9 * * *").desired_width(f32::INFINITY));}
                            else{ui.horizontal_wrapped(|ui|{ui.label("At");ui.add(egui::TextEdit::singleline(&mut editor.time).desired_width(70.0));ui.label("24-hour time");});if kind==Frequency::Weekly{let days=["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"];egui::ComboBox::from_id_salt("weekday").selected_text(days[editor.weekday]).show_ui(ui,|ui|{for (i,day) in days.iter().enumerate(){ui.selectable_value(&mut editor.weekday,i,*day);}});}}
                            ui.horizontal_wrapped(|ui|{ui.label("Timezone");ui.add(egui::TextEdit::singleline(&mut editor.timezone).desired_width(150.0));});
                        }
                    }
                    let agent_turn=editor.original.as_ref().is_none_or(|j|j["payload"]["kind"]=="agentTurn");
                    if agent_turn{ui.add_space(8.0);ui.horizontal_wrapped(|ui|{
                        ui.label("Model");egui::ComboBox::from_id_salt("task_model").width(160.0).selected_text(if editor.model.is_empty(){"Gateway default"}else{&editor.model}).show_ui(ui,|ui|{if editor.model.is_empty(){ui.label("Gateway default");}for model in models{ui.selectable_value(&mut editor.model,model.clone(),model);}});
                    });ui.horizontal_wrapped(|ui|{ui.label("Reasoning");egui::ComboBox::from_id_salt("task_thinking").selected_text(if editor.thinking.is_empty(){"Gateway default"}else{&editor.thinking}).show_ui(ui,|ui|{for value in ["low","medium","high","xhigh","max"]{ui.selectable_value(&mut editor.thinking,value.into(),value);}});});}
                    ui.add_space(8.0);ui.checkbox(&mut editor.enabled,"Enable this schedule");
                    ui.label(egui::RichText::new(if editor.original.is_none(){"Runs independently on the server. Results are saved in run history; no external delivery."}else{"Existing task routing and delivery settings are preserved."}).small().weak());
                    if !self.form_error.is_empty(){ui.colored_label(egui::Color32::LIGHT_RED,&self.form_error);}
                    ui.add_space(10.0);ui.horizontal_wrapped(|ui|{
                        cancel=ui.add_enabled(!busy,egui::Button::new("Cancel")).clicked();
                        submit=ui.add_enabled(connected&&!busy,egui::Button::new(if busy{"Saving…"}else{"Save task"})).clicked();
                    });
                });
            });
        cancel |= !open || ui.input(|i| i.key_pressed(egui::Key::Escape));
        if !cancel {
            if submit {
                match editor.request() {
                    Ok((method, params)) => {
                        self.form_error.clear();
                        self.error.clear();
                        self.queue(method, params, Pending::Save);
                    }
                    Err(error) => self.form_error = error,
                }
            }
            self.editor = Some(editor);
        }
    }
}
pub fn task_summary(result: &Value) -> Option<String> {
    let jobs = task_results(result);
    let job = jobs.first()?;
    Some(format!(
        "{}\n{}\n{}",
        job["name"].as_str().unwrap_or("Task"),
        schedule_label(&job["schedule"]),
        if job["enabled"] == true {
            "Active"
        } else {
            "Paused"
        }
    ))
}
pub fn is_scheduler_tool(name: &str) -> bool {
    name == "automations"
        || name == "cron"
        || name.ends_with("__automations")
        || name.ends_with("__cron")
}
fn task_results(result: &Value) -> Vec<Value> {
    if result["isError"] == true {
        return vec![];
    }
    let mut candidates = vec![result.clone()];
    for key in ["details", "structuredContent", "job"] {
        if result[key].is_object() {
            candidates.push(result[key].clone());
        }
    }
    for content in result["content"].as_array().into_iter().flatten() {
        if let Some(text) = content["text"].as_str().filter(|t| t.len() < 262_144) {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                candidates.push(value);
            }
        }
    }
    let mut jobs = Vec::new();
    for candidate in candidates {
        let rows = if candidate["jobs"].is_array() {
            candidate["jobs"].as_array().unwrap().clone()
        } else if candidate["job"].is_object() {
            vec![candidate["job"].clone()]
        } else {
            vec![candidate]
        };
        for row in rows {
            if row["id"].is_string()
                && row["name"].is_string()
                && row["schedule"].is_object()
                && !jobs.iter().any(|j: &Value| j["id"] == row["id"])
            {
                jobs.push(row);
            }
        }
    }
    jobs
}
fn schedule_label(schedule: &Value) -> String {
    match schedule["kind"].as_str() {
        Some("every") => {
            let ms = schedule["everyMs"].as_u64().unwrap_or(0);
            for (unit, label) in [
                (86_400_000, "day"),
                (3_600_000, "hour"),
                (60_000, "minute"),
                (1000, "second"),
            ] {
                if ms >= unit && ms % unit == 0 {
                    return format!(
                        "Every {} {label}{}",
                        ms / unit,
                        if ms / unit == 1 { "" } else { "s" }
                    );
                }
            }
            format!("Every {ms} ms")
        }
        Some("at") => format!("Once · {}", schedule["at"].as_str().unwrap_or("")),
        Some("cron") => {
            let expr = schedule["expr"].as_str().unwrap_or("");
            let fields: Vec<_> = expr.split_whitespace().collect();
            if fields.len() == 5 && fields[2] == "*" && fields[3] == "*" {
                if let (Ok(minute), Ok(hour)) = (fields[0].parse::<u8>(), fields[1].parse::<u8>()) {
                    let tz = schedule["tz"].as_str().unwrap_or("Gateway timezone");
                    if minute < 60 && hour < 24 {
                        if fields[4] == "*" {
                            return format!("Daily at {hour:02}:{minute:02} · {tz}");
                        }
                        if let Ok(day) = fields[4].parse::<usize>() {
                            if day < 7 {
                                return format!(
                                    "{} at {hour:02}:{minute:02} · {tz}",
                                    [
                                        "Sunday",
                                        "Monday",
                                        "Tuesday",
                                        "Wednesday",
                                        "Thursday",
                                        "Friday",
                                        "Saturday"
                                    ][day]
                                );
                            }
                        }
                    }
                }
            }
            format!(
                "{} · {}",
                expr,
                schedule["tz"].as_str().unwrap_or("Gateway timezone")
            )
        }
        Some(kind) => format!("Gateway schedule · {kind}"),
        None => "Schedule unavailable".into(),
    }
}
#[cfg(test)]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn timestamp(ms: u64) -> String {
    // Gregorian civil date from days since the Unix epoch; timezone is explicit.
    let seconds = ms / 1000;
    let days = (seconds / 86400) as i64 + 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds / 3600 % 24,
        seconds / 60 % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn explicit_task_owner_wins_over_stale_chat_links() {
        let job = serde_json::json!({"id":"task","owner":{"sessionKey":"chat-a"}});
        assert!(super::belongs_to_session(&job, "chat-a", &[]));
        assert!(!super::belongs_to_session(&job, "chat-b", &["task".into()]));
        assert!(super::belongs_to_session(
            &serde_json::json!({"id":"legacy"}),
            "chat-b",
            &["legacy".into()]
        ));
        assert!(!super::belongs_to_session(
            &serde_json::json!({"id":"other"}),
            "chat-b",
            &[]
        ));
    }
    use super::*;
    #[test]
    fn transcript_hydration_counts_queued_requests() {
        let mut tasks = Tasks::default();
        for n in 0..20 {
            tasks.load_result(&json!({"jobId":"j","runId":n.to_string(),"runAtMs":n}));
        }
        assert_eq!(tasks.outbox.len(), 8);
    }
    #[test]
    fn scheduler_tool_results_link_cards_to_their_chat() {
        let job = json!({"id":"task-1","name":"Morning summary","schedule":{"kind":"cron","expr":"0 9 * * *","tz":"Asia/Manila"},"enabled":true});
        let result = json!({"content":[{"type":"text","text":job.to_string()}],"details":job});
        let mut tasks = Tasks::default();
        tasks.observe_tool("chat-a", &result);
        assert_eq!(tasks.chat_links["chat-a"], vec!["task-1"]);
        assert_eq!(tasks.jobs.len(), 1);
        tasks.observe_tool("chat-a", &result);
        assert_eq!(tasks.jobs.len(), 1);
        assert!(task_summary(&result).unwrap().contains("Daily at 09:00"));
        tasks.observe_tool("chat-b", &json!({"isError":true,"details":job}));
        assert!(!tasks.chat_links.contains_key("chat-b"));
        assert!(is_scheduler_tool("automations"));
        assert!(is_scheduler_tool("mcp__openclaw__cron"));
    }
    #[test]
    fn validates_intervals_and_clock_values() {
        let mut e = Editor::new("model", "medium");
        e.frequency = Frequency::Every;
        e.interval = "0".into();
        assert!(e.schedule().is_err());
        e.interval = u64::MAX.to_string();
        assert!(e.schedule().is_err());
        e.interval = "2".into();
        assert_eq!(e.schedule().unwrap()["everyMs"], 7_200_000);
        e.frequency = Frequency::Daily;
        e.time = "25:00".into();
        assert!(e.schedule().is_err());
    }
    #[test]
    fn respects_timezone_and_weekdays() {
        let mut e = Editor::new("model", "medium");
        assert_eq!(
            e.schedule().unwrap(),
            json!({"kind":"cron","expr":"0 9 * * *","tz":"Asia/Manila"})
        );
        e.frequency = Frequency::Weekly;
        e.weekday = 6;
        assert_eq!(e.schedule().unwrap()["expr"], "0 9 * * 0");
        e.frequency = Frequency::Once;
        e.at = "2026-11-18T09:00:00".into();
        assert!(e.schedule().is_err());
    }
    #[test]
    fn edit_preserves_anchor_and_routing() {
        let job = json!({"id":"a","name":"Example","enabled":false,"configRevision":"rev1","schedule":{"kind":"every","everyMs":3600000,"anchorMs":123},"payload":{"kind":"agentTurn","message":"Original","model":"special","thinking":"high"},"sessionTarget":"session:private","delivery":{"mode":"announce","to":"unchanged"}});
        let mut e = Editor::from_job(&job).unwrap();
        e.prompt = "Edited".into();
        let (method, p) = e.request().unwrap();
        assert_eq!(method, "cron.update");
        assert_eq!(p["patch"]["schedule"]["anchorMs"], 123);
        assert_eq!(p["expectedConfigRevision"], "rev1");
        assert!(p["patch"]["delivery"].is_null());
        assert!(p["patch"]["sessionTarget"].is_null());
        assert!(p["patch"]["payload"]["model"].is_null());
    }
    #[test]
    fn new_tasks_keep_output_local() {
        let mut e = Editor::new("model", "medium");
        e.name = "Test".into();
        e.prompt = "Do work".into();
        let (_, p) = e.request().unwrap();
        assert_eq!(p["delivery"]["mode"], "none");
        assert_eq!(p["sessionTarget"], "isolated");
    }
    #[test]
    fn failed_save_keeps_form() {
        let mut tasks = Tasks::default();
        tasks.editor = Some(Editor::new("model", "medium"));
        tasks.pending.insert(
            "save".into(),
            Request {
                kind: Pending::Save,
                started: Instant::now(),
            },
        );
        assert!(
            tasks.handle(&json!({"id":"save","ok":false,"error":{"message":"permission denied"}}))
        );
        assert!(tasks.editor.is_some());
        assert_eq!(tasks.form_error, "permission denied");
    }
    #[test]
    fn stale_history_cannot_replace_selected_task() {
        let mut tasks = Tasks::default();
        tasks.history = Some(("new".into(), "New".into()));
        tasks.pending.insert(
            "old_request".into(),
            Request {
                kind: Pending::Runs("old".into(), 0),
                started: Instant::now(),
            },
        );
        tasks.handle(
            &json!({"id":"old_request","ok":true,"payload":{"entries":[{"summary":"Wrong task"}]}}),
        );
        assert!(tasks.runs.is_empty());
    }
    #[test]
    fn timestamps_use_explicit_utc() {
        assert_eq!(timestamp(0), "1970-01-01 00:00 UTC");
        assert_eq!(timestamp(1_709_164_800_000), "2024-02-29 00:00 UTC");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::gateway::{self, Event};
    fn connect() -> Result<gateway::Gateway, String> {
        let url = std::env::var("D4C_LIVE_GATEWAY")
            .map_err(|_| "Set D4C_LIVE_GATEWAY to the paired test gateway".to_string())?;
        let g = gateway::connect(url, String::new(), egui::Context::default());
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if Instant::now() > deadline {
                return Err("Gateway connection timed out".into());
            }
            match g.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Event::Status(s)) if s == "Connected" => return Ok(g),
                Ok(Event::Error(error)) => return Err(error),
                _ => {}
            }
        }
    }
    fn call(g: &gateway::Gateway, method: &str, params: Value) -> Result<Value, String> {
        let request = gateway::rpc(method, params);
        let id = request["id"].as_str().unwrap().to_owned();
        g.tx.send(request).map_err(|_| "Gateway disconnected")?;
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if Instant::now() > deadline {
                return Err(format!("{method} timed out"));
            }
            match g.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Event::Data(v)) if v["id"] == id => {
                    return if v["ok"] == true {
                        Ok(v["payload"].clone())
                    } else {
                        Err(format!("{method}: {}", v["error"]["message"]))
                    };
                }
                Ok(Event::Error(error)) => return Err(error),
                _ => {}
            }
        }
    }
    fn check(condition: bool, message: &str) -> Result<(), String> {
        if condition {
            Ok(())
        } else {
            Err(message.into())
        }
    }
    fn wait_run(g: &gateway::Gateway, id: &str, after: u64) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let payload = call(g, "cron.runs", json!({"id":id,"limit":10}))?;
            if let Some(run) = payload["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["runAtMs"].as_u64().unwrap_or(0) >= after)
            {
                if run["status"] != "ok" {
                    return Err(format!(
                        "Task did not succeed: {}",
                        run["error"].as_str().unwrap_or("unknown error")
                    ));
                }
                check(
                    run["summary"]
                        .as_str()
                        .unwrap_or("")
                        .contains("D4C_CRON_OK"),
                    "Expected marker missing from run output",
                )?;
                return Ok(run.clone());
            }
            if Instant::now() > deadline {
                return Err("No completed scheduled run before timeout".into());
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    }
    #[test]
    #[ignore = "requires paired live gateway; creates and removes its own verification job"]
    fn live_scheduler_lifecycle() {
        let g = connect().expect("Connect to the paired gateway");
        let name = format!("D4C verification {}", uuid::Uuid::new_v4());
        println!("Checking scheduler status");
        let status = call(&g, "cron.status", json!({})).unwrap();
        assert_eq!(status["enabled"], true, "Live scheduler must be enabled");
        let mut editor = Editor::new("openai/gpt-6.1-terra", "low");
        editor.name = name;
        editor.prompt =
            "Reply exactly D4C_CRON_OK. Do not use tools or send messages to anyone.".into();
        editor.frequency = Frequency::Every;
        editor.interval = "1".into();
        editor.enabled = false;
        let (_, params) = editor.request().unwrap();
        println!("Creating a disabled verification task");
        let job = call(&g, "cron.add", params).expect("Create test task");
        let id = job["id"]
            .as_str()
            .or_else(|| job["job"]["id"].as_str())
            .expect("Created task ID")
            .to_owned();
        let result = (|| -> Result<Value, String> {
            let list = call(&g, "cron.list", json!({"includeDisabled":true,"limit":100}))?;
            check(
                list["jobs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|j| j["id"] == id && j["enabled"] == false),
                "Disabled task not visible",
            )?;
            println!("Editing task, enabling it, and pausing it");
            let updated = call(
                &g,
                "cron.update",
                json!({"id":id,"patch":{"name":"D4C verification · edited","enabled":true,"schedule":{"kind":"every","everyMs":7_200_000}}}),
            )?;
            check(updated["enabled"] == true, "Enable did not persist")?;
            let paused = call(
                &g,
                "cron.update",
                json!({"id":id,"patch":{"enabled":false}}),
            )?;
            check(paused["enabled"] == false, "Pause did not persist")?;
            println!("Running the paused verification task explicitly");
            let manual_start = now_ms();
            call(&g, "cron.run", json!({"id":id,"mode":"force"}))?;
            let manual = wait_run(&g, &id, manual_start)?;
            println!("Manual run succeeded; scheduling a real one-shot run");
            let due = now_ms() + 40_000;
            let at = format!(
                "{}:{:02}Z",
                timestamp(due).replace(" UTC", "").replace(' ', "T"),
                due / 1000 % 60
            );
            call(
                &g,
                "cron.update",
                json!({"id":id,"patch":{"enabled":true,"schedule":{"kind":"at","at":at}}}),
            )?;
            let scheduled = wait_run(&g, &id, due - 1000)?;
            println!("Scheduled run succeeded; checking persisted history");
            let entries = call(&g, "cron.runs", json!({"id":id,"limit":10}))?;
            check(
                entries["entries"]
                    .as_array()
                    .is_some_and(|rows| rows.len() >= 2),
                "Expected manual and scheduled history entries",
            )?;
            let transcript = if let Some(run) = scheduled["runId"].as_str() {
                call(&g, "cron.history", json!({"id":id,"runId":run,"limit":100}))?
            } else {
                json!({})
            };
            Ok(
                json!({"manual_status":manual["status"],"scheduled_status":scheduled["status"],"history_entries":entries["entries"].as_array().map(Vec::len),"transcript_messages":transcript["messages"].as_array().map(Vec::len)}),
            )
        })();
        println!("Removing the verification task");
        let removed = call(&g, "cron.remove", json!({"id":id}));
        let absent = call(&g, "cron.list", json!({"includeDisabled":true,"limit":100})).map(|p| {
            !p["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|j| j["id"] == id)
        });
        let report = json!({"job_id":id,"result":result.as_ref().ok(),"error":result.as_ref().err(),"cleanup":removed.as_ref().ok(),"absent_after_cleanup":absent.as_ref().ok()});
        if let Ok(path) = std::env::var("D4C_CRON_REPORT") {
            let _ = std::fs::write(path, serde_json::to_string_pretty(&report).unwrap());
        }
        assert_eq!(
            absent.unwrap(),
            true,
            "Verification job must be absent after cleanup"
        );
        assert!(result.is_ok(), "{result:?}");
        println!("Live scheduler lifecycle passed");
    }
}
