//! Native OpenClaw question lifecycle; ordinary message text never grants authority.
use eframe::egui;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
#[derive(Default)]
pub struct Questions {
    records: Vec<Value>,
    hydrated: bool,
    answers: HashMap<String, BTreeMap<String, Vec<String>>>,
    custom: HashMap<(String, String), String>,
    pending: HashMap<String, String>,
    errors: HashMap<String, String>,
}
impl Questions {
    pub fn has_session(&self, session: &str) -> bool {
        self.records.iter().any(|r| r["sessionKey"] == session)
    }
    pub fn pending_count(&self, session: &str) -> usize {
        self.records
            .iter()
            .filter(|r| {
                r["sessionKey"] == session
                    && r["status"] == "pending"
                    && r["expiresAtMs"]
                        .as_u64()
                        .is_some_and(|t| t > crate::activity::now())
            })
            .count()
    }
    pub fn observe(&mut self, v: &Value) {
        let p = &v["payload"];
        let id = v["id"].as_str().unwrap_or("");
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
                for r in items.iter().take(64) {
                    self.upsert(r);
                }
            }
        } else if v["event"] == "question.requested" {
            self.upsert(p);
        } else if v["event"] == "question.resolved" {
            if let Some(r) = self.records.iter_mut().find(|r| r["id"] == p["id"]) {
                r["status"] = p["status"].clone();
                r["answers"] = p["answers"].clone();
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
        }
    }

    fn upsert(&mut self, r: &Value) {
        if r["id"].as_str().is_none()
            || r["sessionKey"].as_str().is_none()
            || r["questions"]
                .as_array()
                .is_none_or(|q| q.is_empty() || q.len() > 3)
        {
            return;
        }
        if let Some(old) = self.records.iter_mut().find(|i| i["id"] == r["id"]) {
            *old = r.clone();
        } else {
            if self.records.len() >= 64 {
                self.records.remove(0);
            }
            self.records.push(r.clone());
        }
    }
    pub fn disconnected(&mut self) {
        self.pending.clear();
        self.hydrated = false;
        // Keep drafts, but require a fresh server snapshot before showing actionable requests.
        self.records.clear();
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, session: &str, connected: bool) -> Vec<Value> {
        let mut out = Vec::new();
        for r in self
            .records
            .clone()
            .iter()
            .filter(|r| r["sessionKey"] == session)
        {
            let id = r["id"].as_str().unwrap_or("").to_string();
            let status = r["status"].as_str().unwrap_or("expired");
            let expired = r["expiresAtMs"]
                .as_u64()
                .is_some_and(|t| t <= crate::activity::now());
            let active = status == "pending" && !expired;
            let waiting = self.pending.values().any(|key| key == &id);
            egui::Frame::new().fill(egui::Color32::from_rgb(28,35,38)).corner_radius(12).inner_margin(14).show(ui,|ui|{
                ui.set_width(ui.available_width());
                ui.strong(if active {"Your choice"} else if expired && status=="pending" {"Question expired"} else if status=="answered" {"Answered"} else {"Question closed"});
                let mut unsupported=false;
                let mut complete=true;
                for q in r["questions"].as_array().into_iter().flatten() {
                    let qid=q["questionId"].as_str().unwrap_or("").to_string();
                    ui.add_space(8.0);ui.label(egui::RichText::new(q["question"].as_str().unwrap_or("Question")).color(egui::Color32::from_gray(225)));
                    if !active {
                        if let Some(values)=r["answers"]["answers"][&qid].as_array(){for value in values.iter().filter_map(Value::as_str){let label=q["options"].as_array().and_then(|a|a.iter().find(|o|o["value"]==value || o["label"]==value)).and_then(|o|o["label"].as_str()).unwrap_or(value);ui.label(egui::RichText::new(label).color(egui::Color32::from_rgb(139,216,193)));}}
                        continue;
                    }
                    if q["isSecret"]==true || q.get("resource").is_some_and(|v|!v.is_null()) || q.get("secretStore").is_some_and(|v|!v.is_null()) {
                        unsupported=true;ui.label(egui::RichText::new("Open this question in OpenClaw's web interface.").weak());continue;
                    }
                    let selected=self.answers.entry(id.clone()).or_default().entry(qid.clone()).or_default();
                    ui.add_enabled_ui(active && connected && self.hydrated && !waiting,|ui|{
                        ui.vertical(|ui|{
                            for option in q["options"].as_array().into_iter().flatten().take(64) {
                                let label=option["label"].as_str().unwrap_or("Option");
                                let value=option["value"].as_str().unwrap_or(label).to_owned();
                                if ui.add_sized([ui.available_width(),34.0],egui::Button::selectable(selected.contains(&value),label).fill(if selected.contains(&value){egui::Color32::from_rgb(37,83,72)}else{egui::Color32::from_rgb(36,40,44)}).stroke(egui::Stroke::new(1.0,egui::Color32::from_rgb(65,72,78)))).on_hover_text(option["description"].as_str().unwrap_or("")).clicked() {
                                    if selected.contains(&value){selected.retain(|s|s!=&value);}else{if q["multiSelect"]!=true{selected.clear();}selected.push(value);}
                                }
                            }
                            if q["isOther"]==true || q["options"].as_array().is_none_or(|v|v.is_empty()) {
                                let text=self.custom.entry((id.clone(),qid.clone())).or_default();
                                ui.add(egui::TextEdit::multiline(text).desired_width(f32::INFINITY).desired_rows(2).char_limit(4096).hint_text("Your answer…"));
                            }
                        });
                    });
                    let custom=self.custom.get(&(id.clone(),qid.clone())).is_some_and(|v|!v.trim().is_empty());
                    if selected.is_empty() && !custom && q["allowEmpty"]!=true {complete=false;}
                }
                if let Some(error)=self.errors.get(&id){ui.colored_label(egui::Color32::LIGHT_RED,error);}
                if active {
                    ui.add_space(8.0);ui.horizontal(|ui|{
                        if ui.add_enabled(connected && self.hydrated && !waiting && complete && !unsupported,egui::Button::new(if waiting {"Sending…"}else{"Submit"})).clicked(){
                            let mut answers=self.answers.get(&id).cloned().unwrap_or_default();
                            for q in r["questions"].as_array().into_iter().flatten(){let qid=q["questionId"].as_str().unwrap_or("");if let Some(text)=self.custom.get(&(id.clone(),qid.to_owned())).filter(|t|!t.trim().is_empty()) {let a=answers.entry(qid.to_owned()).or_default();if q["multiSelect"]!=true {a.clear();}if q["answerFormat"]=="lines" {a.extend(text.lines().map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned));}else {a.push(text.trim().to_owned());}}}
                            let request=crate::gateway::rpc("question.resolve",json!({"id":id,"answers":{"answers":answers},"resolutionId":uuid::Uuid::new_v4().to_string(),"resolvedBy":"D4C"}));
                            self.pending.insert(request["id"].as_str().unwrap().into(),id.clone());out.push(request);
                        }
                        if ui.add_enabled(connected && self.hydrated && !waiting,egui::Button::new("Cancel")).clicked(){let request=crate::gateway::rpc("question.resolve",json!({"id":id,"cancel":true,"resolvedBy":"D4C"}));self.pending.insert(request["id"].as_str().unwrap().into(),id.clone());out.push(request);}
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                }
            });
            ui.add_space(10.0);
        }
        out
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
        assert!(!q.has_session("chat-b"));
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
        assert!(!q.has_session("chat-a"));
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
