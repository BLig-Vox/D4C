//! Native Codex question lifecycle; ordinary message text never grants authority.
use eframe::egui;
use serde_json::{Value, json};
use shadcn_egui::components::{
    button::{Button, ButtonVariant},
    card::{Card, CardSize, card_title},
};
use std::collections::{BTreeMap, HashMap, HashSet};

fn is_active(record: &Value) -> bool {
    record["status"] == "pending"
        && record["expiresAtMs"]
            .as_u64()
            .is_none_or(|t| t > crate::activity::now())
}

#[derive(Default)]
pub struct Questions {
    records: Vec<Value>,
    hydrated: bool,
    answers: HashMap<String, BTreeMap<String, Vec<String>>>,
    custom: HashMap<(String, String), String>,
    pending: HashMap<String, String>,
    errors: HashMap<String, String>,
    anchors: HashMap<String, String>,
    markdown: egui_commonmark::CommonMarkCache,
}
impl Questions {
    pub fn pending_count(&self, session: &str) -> usize {
        self.records
            .iter()
            .filter(|r| r["sessionKey"] == session && is_active(r))
            .count()
    }
    #[cfg(test)]
    fn observe(&mut self, v: &Value) {
        self.observe_after(v, "", None);
    }
    pub fn observe_after(&mut self, v: &Value, session: &str, last_item: Option<&str>) {
        let p = &v["payload"];
        let id = v["id"].as_str().unwrap_or("");
        if !id.starts_with("question.")
            && !v["event"]
                .as_str()
                .is_some_and(|s| s.starts_with("question."))
        {
            return;
        }
        if let Some(key) = self.pending.remove(id) {
            if v["ok"] == true {
                if let Some(r) = self.records.iter_mut().find(|r| r["id"] == key) {
                    r["status"] = p["status"].clone();
                    r["answers"] = p["answers"].clone();
                }
                self.errors.remove(&key);
                self.answers.remove(&key);
                self.custom.retain(|(request, _), _| request != &key);
            } else {
                self.errors.insert(
                    key,
                    v["error"]["message"]
                        .as_str()
                        .unwrap_or("Reply failed")
                        .into(),
                );
            }
        }
        if id.starts_with("question.list:") && v["ok"] == true {
            if let Some(items) = p["questions"].as_array() {
                self.hydrated = true;
                // A reconnect snapshot is authoritative for currently pending requests.
                self.records.retain(|r| {
                    r["status"] != "pending" || items.iter().any(|i| i["id"] == r["id"])
                });
                for r in items {
                    self.upsert(r);
                }
            }
        } else if v["event"] == "question.requested" {
            if p["sessionKey"] == session
                && let (Some(id), Some(anchor)) = (p["id"].as_str(), last_item)
            {
                self.anchors
                    .entry(id.into())
                    .or_insert_with(|| anchor.into());
            }
            self.upsert(p);
        } else if v["event"] == "question.resolved" {
            if let Some(r) = self.records.iter_mut().find(|r| r["id"] == p["id"]) {
                if r["status"] == "pending" {
                    r["status"] = p["status"].clone();
                    r["answers"] = p["answers"].clone();
                }
            }
        }
        if self.hydrated {
            let live: std::collections::HashSet<_> = self
                .records
                .iter()
                .filter(|r| r["status"] == "pending")
                .filter_map(|r| r["id"].as_str().map(str::to_owned))
                .collect();
            self.answers.retain(|id, _| live.contains(id));
            self.custom.retain(|(id, _), _| live.contains(id));
            self.errors.retain(|id, _| live.contains(id));
            let ids: HashSet<_> = self
                .records
                .iter()
                .filter_map(|r| r["id"].as_str())
                .collect();
            self.anchors.retain(|id, _| ids.contains(id.as_str()));
        }
    }

    fn upsert(&mut self, r: &Value) {
        if r["id"].as_str().is_none()
            || r["sessionKey"].as_str().is_none()
            || r["questions"].as_array().is_none_or(|q| q.is_empty())
        {
            return;
        }
        if let Some(old) = self.records.iter_mut().find(|i| i["id"] == r["id"]) {
            if old["status"] == "pending" || r["status"] != "pending" {
                *old = r.clone();
            }
        } else {
            self.records.push(r.clone());
        }
    }
    pub fn forget_session(&mut self, session: &str) {
        let removed: HashSet<_> = self
            .records
            .iter()
            .filter(|r| r["sessionKey"] == session)
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        self.records.retain(|r| r["sessionKey"] != session);
        self.pending.retain(|_, id| !removed.contains(id));
        self.answers.retain(|id, _| !removed.contains(id));
        self.custom.retain(|(id, _), _| !removed.contains(id));
        self.errors.retain(|id, _| !removed.contains(id));
        self.anchors.retain(|id, _| !removed.contains(id));
    }
    pub fn disconnected(&mut self) {
        for (_, key) in self.pending.drain() {
            self.errors
                .insert(key, "Connection lost before delivery was confirmed.".into());
        }
        self.hydrated = false;
    }

    pub fn delivery_failed(&mut self, request: &Value) {
        if let Some(id) = request["id"].as_str()
            && let Some(key) = self.pending.remove(id)
        {
            self.errors
                .insert(key, "Couldn’t send your reply. Try again.".into());
        }
    }

    /// Group cards once per frame. Keys borrow the message IDs already in the transcript.
    pub fn cards_by_anchor<'a>(
        &self,
        session: &str,
        message_ids: &HashSet<&'a str>,
    ) -> HashMap<Option<&'a str>, Vec<Value>> {
        let mut by_anchor = HashMap::<_, Vec<_>>::new();
        for record in self.records.iter().filter(|r| r["sessionKey"] == session) {
            let native = record["itemId"].as_str().map(|id| format!("tool:{id}"));
            let anchor = native
                .as_deref()
                .and_then(|id| message_ids.get(id))
                .or_else(|| {
                    let saved = self.anchors.get(record["id"].as_str()?)?;
                    message_ids.get(saved.as_str())
                })
                .copied();
            if anchor.is_none() && record["approval"] == true && !is_active(record) {
                continue;
            }
            by_anchor.entry(anchor).or_default().push(record.clone());
        }
        by_anchor
    }

    pub fn ui_after(
        &mut self,
        ui: &mut egui::Ui,
        connected: bool,
        cards: Vec<Value>,
    ) -> Vec<Value> {
        let mut requests = Vec::new();
        for record in cards {
            let id = record["id"].as_str().unwrap_or_default();
            if record["approval"] == true && !is_active(&record) {
                let label = match record["status"].as_str() {
                    Some("answered") => "Approval answered",
                    Some("cancelled") => "Approval cancelled",
                    Some("pending") => "Approval expired",
                    _ => "Approval closed",
                };
                egui::CollapsingHeader::new(label)
                    .id_salt(("approval_history", id))
                    .show(ui, |ui| {
                        self.card(ui, &record, connected);
                    });
                ui.add_space(8.0);
                continue;
            }
            let reply = ui
                .push_id(("decision", id), |ui| self.card(ui, &record, connected))
                .inner;
            if let Some(cancel) = reply {
                let request = self.reply(&record, cancel);
                self.pending
                    .insert(request["id"].as_str().unwrap().into(), id.into());
                self.errors.remove(id);
                requests.push(request);
            }
            ui.add_space(8.0);
        }
        requests
    }

    fn reply(&self, record: &Value, cancel: bool) -> Value {
        let id = record["id"].as_str().unwrap_or_default();
        let mut params =
            json!({"id":id,"resolvedBy":"D4C","resolutionId":uuid::Uuid::new_v4().to_string()});
        if cancel {
            params["cancel"] = true.into();
        } else {
            let mut answers = self.answers.get(id).cloned().unwrap_or_default();
            for question in record["questions"].as_array().into_iter().flatten() {
                let field = question["questionId"].as_str().unwrap_or_default();
                let Some(text) = self
                    .custom
                    .get(&(id.to_owned(), field.to_owned()))
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                else {
                    continue;
                };
                let selected = answers.entry(field.to_owned()).or_default();
                if question["multiSelect"] != true {
                    selected.clear();
                }
                if question["answerFormat"] == "lines" {
                    selected.extend(
                        text.lines()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                    );
                } else {
                    selected.push(text.to_owned());
                }
            }
            params["answers"] = json!({"answers":answers});
        }
        crate::gateway::rpc("question.resolve", params)
    }

    fn card(&mut self, ui: &mut egui::Ui, r: &Value, connected: bool) -> Option<bool> {
        let id = r["id"].as_str().unwrap_or_default();
        let status = r["status"].as_str().unwrap_or_default();
        let active = is_active(r);
        let waiting = self.pending.values().any(|key| key == id);
        let enabled = connected && self.hydrated && !waiting;
        let questions = r["questions"].as_array()?;
        let mut action = None;
        Card::new().size(CardSize::Sm).show(ui, |ui| {
            let title = match status {
                "answered" => "You answered",
                "cancelled" => "Cancelled",
                "pending" if !active => "Request expired",
                "pending" if r["approval"] == true => "Approval requested",
                "pending" => "Your choice",
                _ => "Request closed",
            };
            card_title(ui, title);
            let mut complete = true;
            let mut show_submit = questions.len() > 1;
            for q in questions {
                let field = q["questionId"].as_str().unwrap_or_default();
                let text = q["question"].as_str().unwrap_or("Question");
                if let Some(header) = q["header"].as_str().filter(|h| !h.is_empty() && *h != text) {
                    ui.strong(header);
                }
                let width = ui.available_width().max(1.0) as usize;
                egui_commonmark::CommonMarkViewer::new()
                    .default_width(Some(width))
                    .max_image_width(Some(width))
                    .show(ui, &mut self.markdown, &crate::links::markdown(text));
                for detail in q["details"].as_array().into_iter().flatten() {
                    let text = detail["text"].as_str().unwrap_or_default();
                    let label = detail["label"].as_str().unwrap_or("Details");
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(label);
                        if ui
                            .small_button("Copy")
                            .on_hover_text(format!("Copy {label}"))
                            .clicked()
                        {
                            ui.copy_text(text.to_owned());
                        }
                    });
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).monospace())
                            .wrap()
                            .selectable(true),
                    );
                }
                let options = q["options"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if !active {
                    if let Some(values) = r["answers"]["answers"][field].as_array() {
                        for value in values.iter().filter_map(Value::as_str) {
                            let label = options
                                .iter()
                                .find(|o| o["value"] == value || o["label"] == value)
                                .and_then(|o| o["label"].as_str())
                                .unwrap_or(value);
                            ui.add(
                                egui::Label::new(egui::RichText::new(label).strong())
                                    .wrap()
                                    .selectable(true),
                            );
                        }
                    }
                    continue;
                }
                if q["isSecret"] == true || !q["resource"].is_null() || !q["secretStore"].is_null()
                {
                    complete = false;
                    ui.weak("Answer this protected question in Codex.");
                    continue;
                }
                let selected = self
                    .answers
                    .entry(id.into())
                    .or_default()
                    .entry(field.into())
                    .or_default();
                let custom = self.custom.entry((id.into(), field.into())).or_default();
                let multiple = q["multiSelect"] == true;
                let freeform =
                    q["isOther"] == true || (options.is_empty() && r["approval"] != true);
                show_submit |= freeform || multiple;
                ui.add_enabled_ui(enabled, |ui| {
                    for option in options {
                        let Some(label) = option["label"].as_str() else {
                            continue;
                        };
                        let value = option["value"].as_str().unwrap_or(label);
                        let checked = selected.iter().any(|s| s == value);
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            label,
                            0.0,
                            egui::TextFormat::simple(
                                egui::TextStyle::Button.resolve(ui.style()),
                                ui.visuals().text_color(),
                            ),
                        );
                        if let Some(description) =
                            option["description"].as_str().filter(|s| !s.is_empty())
                        {
                            job.append(
                                &format!("\n{description}"),
                                0.0,
                                egui::TextFormat::simple(
                                    egui::TextStyle::Small.resolve(ui.style()),
                                    ui.visuals().weak_text_color(),
                                ),
                            );
                        }
                        if ui
                            .add(
                                egui::Button::new(job)
                                    .selected(checked)
                                    .frame(true)
                                    .wrap()
                                    .min_size(egui::vec2(ui.available_width(), 44.0)),
                            )
                            .clicked()
                        {
                            if multiple && checked {
                                selected.retain(|s| s != value);
                            } else {
                                if !multiple {
                                    selected.clear();
                                    custom.clear();
                                }
                                selected.push(value.to_owned());
                            }
                            if questions.len() == 1 && !multiple {
                                action = Some(false);
                            }
                        }
                    }
                    if freeform {
                        if ui
                            .add(
                                egui::TextEdit::multiline(custom)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(2)
                                    .char_limit(4096)
                                    .hint_text("Your answer…"),
                            )
                            .changed()
                            && !multiple
                            && !custom.trim().is_empty()
                        {
                            selected.clear();
                        }
                    }
                });
                complete &=
                    !selected.is_empty() || !custom.trim().is_empty() || q["allowEmpty"] == true;
            }
            if let Some(error) = self.errors.get(id) {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if active {
                ui.horizontal_wrapped(|ui| {
                    if waiting {
                        ui.spinner();
                        ui.label("Sending…");
                    } else if show_submit
                        && ui
                            .add_enabled(enabled && complete, Button::new("Send answer"))
                            .clicked()
                    {
                        action = Some(false);
                    }
                    if ui
                        .add_enabled(enabled, Button::new("Cancel").variant(ButtonVariant::Ghost))
                        .clicked()
                    {
                        action = Some(true);
                    }
                });
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Value {
        json!({"type":"event","event":"question.requested","payload":{"id":"q1","sessionKey":"chat-a","status":"pending","expiresAtMs":crate::activity::now()+60000,"questions":[{"questionId":"q","question":"Proceed?","options":[{"label":"Yes"},{"label":"No"}]}]}})
    }
    #[test]
    fn reconnect_preserves_answer_draft_until_authoritative_snapshot() {
        let mut q = Questions::default();
        let event = request();
        q.observe(&json!({"id":"question.list:0","ok":true,"payload":{"questions":[event["payload"].clone()]}}));
        q.custom
            .insert(("q1".into(), "q".into()), "custom draft".into());
        q.disconnected();
        q.observe(&json!({"type":"res","id":"connect","ok":true,"payload":{}}));
        assert_eq!(q.custom.len(), 1);
        q.observe(&json!({"id":"question.list:1","ok":true,"payload":{"questions":[event["payload"].clone()]}}));
        assert_eq!(q.custom[&("q1".into(), "q".into())], "custom draft");
    }
    #[test]
    fn requests_are_scoped_and_resolutions_close_them() {
        let mut q = Questions::default();
        q.observe(&request());
        assert_eq!(q.pending_count("chat-a"), 1);
        assert_eq!(q.pending_count("chat-b"), 0);
        assert!(q.cards_by_anchor("chat-b", &HashSet::new()).is_empty());
        q.observe(&json!({"event":"question.resolved","payload":{"id":"q1","status":"answered","answers":{"answers":{"q":["Yes"]}}}}));
        assert_eq!(q.pending_count("chat-a"), 0);
        assert_eq!(q.records[0]["answers"]["answers"]["q"][0], "Yes");
    }
    #[test]
    fn failed_reply_keeps_draft_for_retry() {
        let mut q = Questions::default();
        q.observe(&request());
        q.answers
            .entry("q1".into())
            .or_default()
            .insert("q".into(), vec!["No".into()]);
        q.pending.insert("question.resolve:1".into(), "q1".into());
        q.observe(&json!({"type":"res","id":"question.resolve:1","ok":false,"error":{"message":"denied"}}));
        assert_eq!(q.answers["q1"]["q"], vec!["No"]);
        assert_eq!(q.errors["q1"], "denied");
        assert!(q.pending.is_empty());
    }
    #[test]
    fn reconnect_snapshot_removes_missing_pending_requests() {
        let mut q = Questions::default();
        q.observe(&request());
        q.observe(
            &json!({"type":"res","id":"question.list:1","ok":true,"payload":{"questions":[]}}),
        );
        assert!(q.cards_by_anchor("chat-a", &HashSet::new()).is_empty());
    }
    #[test]
    fn settled_approvals_only_appear_with_a_transcript_anchor() {
        let mut q = Questions::default();
        let mut event = request();
        event["payload"]["approval"] = true.into();
        event["payload"]["itemId"] = "command-1".into();
        q.observe(&event);
        assert_eq!(q.cards_by_anchor("chat-a", &HashSet::new())[&None].len(), 1);

        q.observe(&json!({"event":"question.resolved","payload":{"id":"q1","status":"answered"}}));
        assert!(q.cards_by_anchor("chat-a", &HashSet::new()).is_empty());
        let ids = HashSet::from(["tool:command-1"]);
        assert_eq!(q.cards_by_anchor("chat-a", &ids)[&Some("tool:command-1")].len(), 1);
    }
    #[test]
    fn acknowledged_choice_survives_replay_and_reconnect() {
        let mut q = Questions::default();
        let event = request();
        q.observe_after(&event, "chat-a", Some("message:before"));
        q.observe_after(&event, "chat-a", Some("message:later"));
        q.pending.insert("question.resolve:1".into(), "q1".into());
        q.observe(&json!({"id":"question.resolve:1","ok":true,"payload":{"status":"answered","answers":{"answers":{"q":["Yes"]}}}}));
        q.observe(&json!({"event":"question.resolved","payload":{"id":"q1","status":"resolved"}}));
        q.disconnected();
        q.observe(&json!({"id":"question.list:after","ok":true,"payload":{"questions":[]}}));
        q.observe(&event);
        assert_eq!(q.records.len(), 1);
        assert_eq!(q.records[0]["status"], "answered");
        assert_eq!(q.records[0]["answers"]["answers"]["q"][0], "Yes");
        assert_eq!(q.anchors["q1"], "message:before");
    }

    #[test]
    fn unsent_reply_can_be_retried_with_the_same_selection() {
        let mut q = Questions::default();
        let record = request()["payload"].clone();
        q.answers
            .entry("q1".into())
            .or_default()
            .insert("q".into(), vec!["No".into()]);
        let first = q.reply(&record, false);
        q.pending
            .insert(first["id"].as_str().unwrap().into(), "q1".into());
        q.delivery_failed(&first);
        assert!(q.pending.is_empty());
        assert!(q.errors.contains_key("q1"));
        let retry = q.reply(&record, false);
        assert_eq!(retry["params"]["answers"], first["params"]["answers"]);
        assert_ne!(retry["id"], first["id"]);
    }

    #[test]
    fn unscoped_request_is_never_attached_to_selected_chat() {
        let mut q = Questions::default();
        let mut event = request();
        event["payload"]
            .as_object_mut()
            .unwrap()
            .remove("sessionKey");
        q.observe(&event);
        assert!(q.records.is_empty());
    }
}
