//! Translation between D4C's UI events and the Codex app-server protocol.
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(super) enum Output {
    Wire(Value),
    Ui(Value),
}

#[derive(Default)]
pub(super) struct Adapter {
    pub output: Vec<Output>,
    pending: HashMap<String, (String, Value)>,
    // Serialize thread/start and thread/resume with commands that need a live thread.
    opening: HashMap<String, Vec<Value>>,
    loaded: HashSet<String>,
    aliases: HashMap<String, String>,
    active: HashMap<String, String>,
    cwd: HashMap<String, String>,
    skills: HashMap<String, HashMap<String, String>>,
    questions: HashMap<String, (Value, String, Value)>,
    serial: u64,
}

fn string(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn rows(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn key(req: &Value) -> &str {
    let p = &req["params"];
    p["key"]
        .as_str()
        .or_else(|| p["sessionKey"].as_str())
        .unwrap_or("")
}
fn is_thread(key: &str) -> bool {
    uuid::Uuid::parse_str(key).is_ok()
}
fn copy_options(p: &Value, params: &mut Value) {
    if let Some(model) = p["model"].as_str().filter(|s| !s.is_empty()) {
        params["model"] = json!(model.strip_prefix("openai/").unwrap_or(model));
    }
    if let Some(effort) = p["thinkingLevel"].as_str().filter(|s| !s.is_empty()) {
        params["effort"] = json!(if effort == "off" { "none" } else { effort });
    }
}

impl Adapter {
    fn wire(&mut self, v: Value) {
        self.output.push(Output::Wire(v));
    }
    fn event(&mut self, name: &str, p: Value) {
        self.output
            .push(Output::Ui(json!({"type":"event","event":name,"payload":p})));
    }
    fn ok(&mut self, req: &Value, p: Value) {
        self.output.push(Output::Ui(
            json!({"type":"res","id":req["id"],"ok":true,"payload":p}),
        ));
    }
    fn fail(&mut self, req: &Value, msg: &str) {
        self.output.push(Output::Ui(
            json!({"type":"res","id":req["id"],"ok":false,"error":{"message":msg}}),
        ));
    }
    fn call(&mut self, method: &str, params: Value, req: Value) {
        self.serial += 1;
        let id = format!("d4c-{}", self.serial);
        self.pending.insert(id.clone(), (method.into(), req));
        let mut frame = json!({"id":id,"method":method});
        if !params.is_null() {
            frame["params"] = params;
        }
        self.wire(frame);
    }
    fn open(&mut self, k: &str, req: Value) {
        if let Some(queue) = self.opening.get_mut(k) {
            queue.push(req);
            return;
        }
        self.opening.insert(k.into(), vec![req]);
        let context = json!({"params":{"key":k}});
        let mut options = json!({"sandbox":"workspace-write","approvalPolicy":"on-request"});
        if is_thread(k) {
            options = json!({"threadId":k});
            options["excludeTurns"] = json!(true);
            self.call("thread/resume", options, context);
        } else {
            self.call("thread/start", options, context);
        }
    }
    pub fn command(&mut self, mut req: Value) {
        let mut k = key(&req).to_owned();
        if let Some(actual) = self.aliases.get(&k) {
            k = actual.clone();
            if req["params"]["key"].is_string() {
                req["params"]["key"] = json!(k);
            }
            if req["params"]["sessionKey"].is_string() {
                req["params"]["sessionKey"] = json!(k);
            }
        }
        let p = &req["params"];
        let method = string(&req["method"]);
        match method {
            "sessions.list" => {
                let params = json!({"limit":100,"modelProviders":[],"sourceKinds":["cli","vscode","exec","appServer","unknown"],"sortKey":"updated_at","cursor":req["cursor"]});
                self.call("thread/list", params, req);
            }
            "models.list" => {
                let params = json!({"limit":100,"cursor":req["cursor"]});
                self.call("model/list", params, req);
            }
            "sessions.messages.subscribe" if !is_thread(&k) => self.ok(&req, json!({})),
            "sessions.messages.subscribe" | "chat.send" | "sessions.patch" | "permissions.set"
                if !self.loaded.contains(&k) =>
            {
                self.open(&k, req);
            }
            // The connection retains opened threads to receive approvals and completion notifications.
            "sessions.messages.subscribe"
            | "sessions.messages.unsubscribe"
            | "sessions.subscribe" => {
                self.ok(&req, json!({}));
            }
            "chat.history" if !is_thread(&k) => {
                self.ok(
                    &req,
                    json!({"messages":[],"sessionInfo":{"hasActiveRun":false}}),
                );
            }
            "chat.history" => {
                let params = json!({"threadId":k,"itemsView":"full","sortDirection":"desc","limit":20,"cursor":p["cursor"]});
                self.call("thread/turns/list", params, req);
            }
            "permissions.set" => {
                let (sandbox, approval) = match string(&p["mode"]) {
                    "yolo" => ("dangerFullAccess", "never"),
                    "default" => ("workspaceWrite", "on-request"),
                    "read-only" => ("readOnly", "on-request"),
                    _ => {
                        self.fail(&req, "Unknown permission mode");
                        return;
                    }
                };
                self.call("thread/settings/update", json!({"threadId":k,"approvalPolicy":approval,"sandboxPolicy":{"type":sandbox}}), req);
            }
            "sessions.patch" if p["label"].is_string() => {
                let params = json!({"threadId":k,"name":p["label"]});
                self.call("thread/name/set", params, req);
            }
            "sessions.patch" => {
                let mut params = json!({"threadId":k});
                copy_options(p, &mut params);
                self.call("thread/settings/update", params, req);
            }
            "sessions.reload" => self.call("thread/read", json!({"threadId":k}), req),
            "sessions.delete" => self.call("thread/delete", json!({"threadId":k}), req),
            "sessions.archive" => self.call("thread/archive", json!({"threadId":k}), req),
            "chat.send" => self.send_turn(req),
            "artifacts.download" => {
                let mut params = json!({"artifactId":p["artifactId"]});
                if let Some(offset) = p["offset"].as_u64() {
                    params["offset"] = json!(offset);
                }
                self.call("d4c/attachment/download", params, req)
            }
            "chat.abort" => {
                if let Some(turn) = self.active.get(&k) {
                    self.call("turn/interrupt", json!({"threadId":k,"turnId":turn}), req);
                } else {
                    self.call(
                        "thread/turns/list",
                        json!({"threadId":k,"sortDirection":"desc","limit":1}),
                        req,
                    );
                }
            }
            "commands.list" => {
                let mut params = json!({});
                if let Some(cwd) = self.cwd.get(&k) {
                    params["cwds"] = json!([cwd]);
                }
                self.call("skills/list", params, req);
            }
            "question.list" => {
                let questions = self.questions.values().map(|q| &q.2).collect::<Vec<_>>();
                self.ok(&req, json!({"questions":questions}));
            }
            "question.resolve" => self.resolve(req),
            method if method.starts_with("cron.") => {
                let route = format!("d4c/cron/{}", &method[5..]);
                self.call(&route, p.clone(), req);
            }
            _ => self.fail(&req, &format!("The Codex server does not support {method}")),
        }
    }

    fn send_turn(&mut self, mut req: Value) {
        let k = key(&req).to_owned();
        let p = &req["params"];
        let mut input = vec![];
        let text = string(&p["message"]);
        if !text.is_empty() {
            input.push(json!({"type":"text","text":text}));
            if let Some(name) = text
                .split_whitespace()
                .next()
                .and_then(|s| s.strip_prefix('/'))
            {
                let cwd = self.cwd.get(&k).map(String::as_str).unwrap_or("");
                if let Some(path) = self
                    .skills
                    .get(cwd)
                    .or_else(|| self.skills.get(""))
                    .and_then(|m| m.get(name))
                {
                    input.push(json!({"type":"skill","name":name,"path":path}));
                }
            }
        }
        for (i, attachment) in rows(&p["attachments"]).iter().enumerate() {
            let mime = string(&attachment["mimeType"]);
            let data = string(&attachment["content"]);
            let name = string(&attachment["fileName"]);
            if mime.starts_with("image/") || mime.starts_with("audio/") {
                let kind = if mime.starts_with("image/") {
                    "image"
                } else {
                    "audio"
                };
                input.push(json!({"type":kind,"url":format!("data:{mime};base64,{data}")}));
            } else if let Some(path) = attachment["storedPath"].as_str() {
                input.push(json!({"type":"text","text":format!("Attached file {name}: {path}")}));
            } else {
                let args = json!({"name":name,"data":data});
                req["attachmentIndex"] = json!(i);
                self.call("d4c/attachment/store", args, req);
                return;
            }
        }
        let mut args =
            json!({"threadId":k,"input":input,"clientUserMessageId":p["idempotencyKey"]});
        copy_options(p, &mut args);
        self.call("turn/start", args, req);
    }
    pub fn incoming(&mut self, v: Value) {
        if v["method"].is_string() {
            if !v["id"].is_null() {
                self.server_request(v);
            } else {
                self.notify(string(&v["method"]), &v["params"]);
            }
            return;
        }
        let Some((method, mut req)) = self.pending.remove(string(&v["id"])) else {
            return;
        };
        let k = key(&req).to_owned();
        let r = &v["result"];
        let error = v["error"]["message"].as_str();
        if matches!(method.as_str(), "thread/start" | "thread/resume") {
            let queue = self.opening.remove(&k).unwrap_or_default();
            if let Some(id) = r["thread"]["id"].as_str().filter(|_| error.is_none()) {
                self.loaded.insert(id.into());
                self.remember_thread(&r["thread"]);
                if id != k {
                    self.aliases.insert(k.clone(), id.into());
                    self.event("session.created", json!({"previousKey":k,"sessionKey":id}));
                }
                self.event("session.settings",json!({"sessionKey":id,"model":r["model"],"thinkingLevel":r["reasoningEffort"],"sandbox":r["sandbox"]["type"],"approval":r["approvalPolicy"]}));
                for cmd in queue {
                    self.command(cmd);
                }
            } else {
                for cmd in queue {
                    self.fail(&cmd, error.unwrap_or("Codex returned no thread identifier"));
                }
            }
            return;
        }
        if let Some(msg) = error {
            self.fail(&req, msg);
            return;
        }
        match method.as_str() {
            "thread/list" | "model/list" => {
                let mut data = req["data"].as_array().cloned().unwrap_or_default();
                for row in rows(&r["data"]) {
                    if method == "thread/list" {
                        self.remember_thread(row);
                        let title = thread_title(row);
                        data.push(json!({"key":row["id"],"label":title,"model":row["model"],"modelProvider":row["modelProvider"],"thinkingLevel":row["reasoningEffort"],"hasActiveRun":row["status"]["type"]=="active"}));
                    } else {
                        data.push(row.clone());
                    }
                }
                if let Some(cursor) = r["nextCursor"]
                    .as_str()
                    .filter(|s| !s.is_empty() && req["cursor"] != *s)
                {
                    req["cursor"] = json!(cursor);
                    req["data"] = json!(data);
                    self.command(req);
                } else if method == "thread/list" {
                    self.ok(&req, json!({"sessions":data}));
                } else {
                    self.ok(&req, json!({"models":data}));
                }
            }
            "thread/read" if req["method"] == "sessions.reload" => {
                let status = &r["thread"]["status"]["type"];
                if status == "active" || self.active.contains_key(&k) {
                    self.fail(&req, "Wait for the current turn to finish before reloading");
                } else {
                    self.call("config/mcpServer/reload", Value::Null, req);
                }
            }
            "config/mcpServer/reload" => {
                let mut args = json!({"forceReload":true});
                if let Some(cwd) = self.cwd.get(&k) {
                    args["cwds"] = json!([cwd]);
                }
                self.call("skills/list", args, req);
            }
            "skills/list" => {
                let mut commands = vec![];
                for entry in rows(&r["data"]) {
                    let mut paths = HashMap::new();
                    for skill in rows(&entry["skills"])
                        .iter()
                        .filter(|s| s["enabled"] != false)
                    {
                        let name = string(&skill["name"]);
                        paths.insert(name.to_owned(), string(&skill["path"]).to_owned());
                        commands.push(json!({"name":name,"description":skill["description"],"source":"skill"}));
                    }
                    if k.is_empty() {
                        self.skills.insert(String::new(), paths.clone());
                    }
                    self.skills.insert(string(&entry["cwd"]).to_owned(), paths);
                }
                self.ok(&req, json!({"commands":commands}));
            }
            "thread/turns/list" if req["method"] == "chat.abort" => {
                if let Some(turn) = rows(&r["data"])
                    .iter()
                    .find(|t| t["status"] == "inProgress")
                {
                    self.call(
                        "turn/interrupt",
                        json!({"threadId":k,"turnId":turn["id"]}),
                        req,
                    );
                } else {
                    self.fail(&req, "There is no active turn to interrupt");
                }
            }
            "thread/turns/list" => {
                let turns = rows(&r["data"]);
                let mut info = json!({});
                if req["params"]["cursor"].is_null() {
                    let last = turns.first().unwrap_or(&Value::Null);
                    info = json!({"hasActiveRun":last["status"]=="inProgress","status":last["status"],"lastRunId":last["id"],"lastRunError":last["error"]["message"]});
                    if last["status"] == "inProgress" {
                        self.active.insert(k.clone(), string(&last["id"]).into());
                    }
                }
                let messages: Vec<_> = turns
                    .iter()
                    .rev()
                    .flat_map(|turn| rows(&turn["items"]).iter().filter_map(transcript))
                    .collect();
                self.ok(
                    &req,
                    json!({"messages":messages,"sessionInfo":info,"olderCursor":r["nextCursor"]}),
                );
            }
            "d4c/attachment/store" => {
                if let (Some(i), Some(path)) = (req["attachmentIndex"].as_u64(), r["path"].as_str())
                {
                    req["params"]["attachments"][i as usize]["storedPath"] = json!(path);
                    self.send_turn(req);
                } else {
                    self.fail(&req, "Codex server did not return an attachment path");
                }
            }
            "turn/start" => {
                if r["turn"]["status"] == "inProgress" {
                    self.active.insert(k, string(&r["turn"]["id"]).into());
                }
                self.ok(&req, json!({"runId":r["turn"]["id"]}));
            }
            _ => self.ok(&req, r.clone()),
        }
    }

    fn remember_thread(&mut self, t: &Value) {
        if let (Some(k), Some(cwd)) = (t["id"].as_str(), t["cwd"].as_str()) {
            self.cwd.insert(k.into(), cwd.into());
        }
    }
    fn notify(&mut self, method: &str, p: &Value) {
        let k = string(&p["threadId"]);
        let turn = &p["turn"];
        let item = &p["item"];
        match method{
            "thread/started"=>{self.remember_thread(&p["thread"]);self.event("sessions.changed",json!({"sessionKey":p["thread"]["id"]}));}
            "thread/settings/updated" => {
                let settings = &p["threadSettings"];
                self.event("session.settings",json!({"sessionKey":k,"model":settings["model"],"thinkingLevel":settings["effort"],"sandbox":settings["sandboxPolicy"]["type"],"approval":settings["approvalPolicy"]}));
            }
            "thread/name/updated"|"thread/unarchived"|"thread/status/changed"=>{
                if method=="thread/status/changed" && p["status"]["type"]!="active"{self.active.remove(k);}
                self.event("sessions.changed",json!({"sessionKey":k}));
            }
            "thread/closed"=>{self.loaded.remove(k);self.active.remove(k);}
            "thread/deleted"|"thread/archived"=>{
                self.loaded.remove(k);self.active.remove(k);
                self.event("sessions.changed",json!({"sessionKey":k,"reason":"delete"}));
            }
            "skills/changed"=>self.event("skills.changed",json!({})),
            "turn/started"=>{
                self.active.insert(k.into(),string(&turn["id"]).into());
                self.event("chat",json!({"sessionKey":k,"state":"delta","runId":turn["id"]}));
            }
            "turn/completed"=>{
                self.active.remove(k);
                let state=match string(&turn["status"]){"failed"=>"error","interrupted"=>"aborted",_=>"final"};
                self.event("chat",json!({"sessionKey":k,"state":state,"runId":turn["id"],"errorMessage":turn["error"]["message"]}));
            }
            "error" if p["willRetry"]!=true=>self.event("chat",json!({"sessionKey":k,"state":"error","runId":p["turnId"],"errorMessage":p["error"]["message"]})),
            "item/agentMessage/delta"=>self.event("chat",json!({"sessionKey":k,"state":"delta","runId":p["turnId"],"itemId":p["itemId"],"deltaText":p["delta"]})),
            "item/reasoning/summaryTextDelta"=>self.event("agent",json!({"sessionKey":k,"stream":"reasoning","runId":p["turnId"],"data":{"itemId":p["itemId"],"delta":p["delta"]}})),
            "item/commandExecution/outputDelta"|"item/fileChange/outputDelta" => {
                let name = if method == "item/commandExecution/outputDelta" {"commandExecution"} else {"fileChange"};
                self.event("agent",json!({"sessionKey":k,"runId":p["turnId"],"stream":"tool","data":{"toolCallId":p["itemId"],"name":name,"phase":"update","result":{"text":p["delta"]}}}));
            }
            "item/started"|"item/completed"=>{
                let done=method=="item/completed";
                match string(&item["type"]){
                    "userMessage"=>if let Some(message)=transcript(item){self.event("session.message",json!({"sessionKey":k,"messageId":item["id"],"message":message}));},
                    "agentMessage" if done=>{
                        if let Some(message)=transcript(item){
                            self.event("chat",json!({"sessionKey":k,"state":"delta","runId":p["turnId"],"itemId":item["id"],"replace":message["content"].is_array(),"message":message}));
                        }
                    }
                    "reasoning" if done=>{
                        let summary=lines(&item["summary"]);
                        if !summary.is_empty(){self.event("agent",json!({"sessionKey":k,"stream":"reasoning","data":{"itemId":item["id"],"text":summary}}));}
                    }
                    "agentMessage"|"reasoning"=>{}
                    _=>{
                        if let Some(message)=transcript(item){
                            self.event("agent",json!({"sessionKey":k,"stream":"tool","runId":p["turnId"],"data":{"toolCallId":item["id"],"name":message["toolName"],"phase":if done{"result"}else{"start"},"isError":message["isError"],"result":if message["content"].is_array() {json!({"content":message["content"]})} else {json!({"text":message["content"]})}}}));
                        }
                    }
                }
            }
            "serverRequest/resolved"=>{
                if let Some(id)=self.questions.iter().find_map(|(id,q)| (q.0==p["requestId"] && q.2["sessionKey"]==p["threadId"]).then(||id.clone())){
                    self.questions.remove(&id);self.event("question.resolved",json!({"id":id,"status":"resolved"}));
                }
            }
            _=>{}
        }
    }

    fn server_request(&mut self, v: Value) {
        let method = string(&v["method"]);
        let p = &v["params"];
        let qs = match method {
            "item/tool/requestUserInput" => rows(&p["questions"])
                .iter()
                .map(|q| {
                    let mut question = q.clone();
                    question["questionId"] = q["id"].clone();
                    question
                })
                .collect(),
            "item/permissions/requestApproval" => {
                let reason = p["reason"]
                    .as_str()
                    .unwrap_or("The turn requested additional permissions.");
                let details = approval_details(
                    p,
                    &[
                        ("Folder", "cwd"),
                        ("Environment", "environmentId"),
                        ("Permissions", "permissions"),
                    ],
                );
                vec![
                    json!({"questionId":"decision","question":reason,"details":details,"options":[{"label":"Allow for this turn","value":"accept"},{"label":"Decline","value":"decline"}]}),
                ]
            }
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let prompt = if method == "item/commandExecution/requestApproval" {
                    "Allow this command?"
                } else {
                    "Allow these file changes?"
                };
                let details = approval_details(
                    p,
                    &[
                        ("Command", "command"),
                        ("Folder", "cwd"),
                        ("Reason", "reason"),
                        ("Write access", "grantRoot"),
                        ("Item", "itemId"),
                        ("Environment", "environmentId"),
                        ("Network", "networkApprovalContext"),
                        ("Additional permissions", "additionalPermissions"),
                        ("Command actions", "commandActions"),
                    ],
                );
                let allowed = &p["availableDecisions"];
                let options: Vec<_> = [("Allow once", "accept"), ("Decline", "decline")]
                    .iter()
                    .filter(|(_, d)| allowed.is_null() || rows(allowed).iter().any(|v| v == *d))
                    .map(|(label, value)| json!({"label":label,"value":value}))
                    .collect();
                vec![
                    json!({"questionId":"decision","question":prompt,"details":details,"options":options}),
                ]
            }
            "currentTime/read" => {
                self.wire(
                    json!({"id":v["id"],"result":{"currentTimeAt":crate::activity::now()/1000}}),
                );
                return;
            }
            _ => {
                self.wire(json!({"id":v["id"],"error":{"code":-32601,"message":format!("D4C does not support {method}")}}));
                return;
            }
        };
        // Retried notifications for the same native request must target one card.
        let id = json!([
            p["threadId"],
            p["turnId"],
            p["itemId"],
            p["approvalId"],
            p["startedAtMs"],
            method,
            v["id"]
        ])
        .to_string();
        if self.questions.contains_key(&id) {
            return;
        }
        let expires = p["autoResolutionMs"]
            .as_u64()
            .map_or(u64::MAX, |ms| crate::activity::now().saturating_add(ms));
        let record = json!({"id":id,"requestedPermissions":p["permissions"],"sessionKey":p["threadId"],"itemId":p["itemId"],"approval":method.ends_with("/requestApproval"),"status":"pending","questions":qs,"expiresAtMs":expires});
        self.questions
            .insert(id, (v["id"].clone(), method.into(), record.clone()));
        self.event("question.requested", record);
    }
    fn resolve(&mut self, req: Value) {
        let p = &req["params"];
        let id = string(&p["id"]);
        let Some((native, method, record)) = self.questions.get(id) else {
            self.fail(&req, "This Codex request has already closed");
            return;
        };
        let cancelled = p["cancel"] == true;
        let answers = &p["answers"]["answers"];
        let result = if method == "item/tool/requestUserInput" {
            let mut values = json!({});
            if !cancelled {
                for q in rows(&record["questions"]) {
                    let field = string(&q["questionId"]);
                    if let Some(answer) = answers[field].as_array() {
                        values[field] = json!({"answers":answer});
                    }
                }
            }
            json!({"answers":values})
        } else {
            let decision = if cancelled {
                "cancel"
            } else {
                string(&answers["decision"][0])
            };
            if !cancelled
                && !rows(&record["questions"][0]["options"])
                    .iter()
                    .any(|o| o["value"] == decision)
            {
                self.fail(&req, "Choose one of the available decisions");
                return;
            }
            if method == "item/permissions/requestApproval" {
                let permissions = if decision == "accept" {
                    record["requestedPermissions"].clone()
                } else {
                    json!({})
                };
                json!({"permissions":permissions,"scope":"turn"})
            } else {
                json!({"decision":decision})
            }
        };
        self.wire(json!({"id":native,"result":result}));
        self.questions.remove(id);
        self.ok(
            &req,
            json!({"status":if cancelled{"cancelled"}else{"answered"},"answers":p["answers"]}),
        );
    }
}

fn approval_details(params: &Value, fields: &[(&str, &str)]) -> Vec<Value> {
    fields
        .iter()
        .filter_map(|(label, key)| {
            let value = params.get(*key).filter(|v| !v.is_null())?;
            let text = value.as_str().map(str::to_owned).unwrap_or_else(|| {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            });
            Some(json!({"label":label,"text":text}))
        })
        .collect()
}

fn thread_title(thread: &Value) -> Option<String> {
    let mut imported = false;
    for field in ["name", "preview"] {
        let Some(mut text) = thread[field].as_str().map(str::trim) else {
            continue;
        };
        loop {
            if text.starts_with("[OpenClaw conversation info:") {
                imported = true;
                let mut depth = 0;
                let mut quoted = false;
                let mut escaped = false;
                let end = text.char_indices().find_map(|(i, ch)| {
                    if escaped {
                        escaped = false;
                    } else if quoted && ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        quoted = !quoted;
                    } else if !quoted && ch == '[' {
                        depth += 1;
                    } else if !quoted && ch == ']' {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    None
                });
                text = end.map_or("", |i| text[i..].trim());
            } else if text.starts_with(
                "D4C scheduled tasks run in isolated sessions with delivery mode none.",
            ) || text
                .starts_with("D4C workspace: no desktop is attached to this conversation.")
            {
                imported = true;
                text = text
                    .split_once('\n')
                    .map_or("", |(_, prompt)| prompt.trim());
            } else if text.starts_with("D4C workspace: a persistent remote desktop") {
                imported = true;
                text = text
                    .split_once("in your final reply.")
                    .map(|(_, prompt)| prompt.trim())
                    .or_else(|| text.split_once("\n\n").map(|(_, prompt)| prompt.trim()))
                    .unwrap_or("");
            } else {
                break;
            }
        }
        if !text.is_empty() {
            return Some(text.chars().take(240).collect());
        }
    }
    imported.then(|| "Imported conversation".into())
}

fn lines(v: &Value) -> String {
    rows(v)
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

// Non-image uploads are passed to Codex as text so it can read the file.
// Reconstruct only D4C upload references; arbitrary host paths never become downloads.
fn upload_part(text: &str) -> Option<Value> {
    let line = text.strip_prefix("Attached file ")?;
    let (_, after) = line.rsplit_once("/uploads/")?;
    let (key, name) = after.split_once('/')?;
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || !line.starts_with(&format!("{name}: /"))
        || name.len() > 240
        || !uuid::Uuid::parse_str(key).is_ok_and(|u| u.to_string() == key)
    {
        return None;
    }
    let mime = crate::media::media_mime(name)?;
    Some(
        json!({"type":"file","fileName":name,"mimeType":mime,"artifactId":format!("upload/{key}/{name}")}),
    )
}
fn item_media(item: &Value) -> Vec<Value> {
    let mut media = Vec::new();
    let mut add = |v: &Value| {
        if v["artifactId"].is_string()
            && v["mimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("image/") || m.starts_with("video/"))
            && !media
                .iter()
                .any(|m: &Value| m["artifactId"] == v["artifactId"])
            && media.len() < 8
        {
            media.push(v.clone());
        }
    };
    add(&item["d4cAttachment"]);
    for part in rows(&item["d4cAttachments"]) {
        add(part);
    }
    for p in rows(&item["contentItems"]) {
        add(&p["d4cAttachment"]);
        if item["tool"] == "share_media"
            && let Some(text) = p["text"].as_str().filter(|s| s.len() < 8192)
            && let Ok(result) = serde_json::from_str::<Value>(text)
        {
            add(&result["d4cAttachment"]);
        }
    }
    media
}
fn with_media(text: Value, mut attachments: Vec<Value>) -> Value {
    if attachments.is_empty() {
        return text;
    }
    if !string(&text).is_empty() {
        attachments.insert(0, json!({"type":"text","text":text}));
    }
    Value::Array(attachments)
}

fn transcript(item: &Value) -> Option<Value> {
    let kind = string(&item["type"]);
    let id = &item["id"];
    match kind {
        "userMessage" => {
            let content:Vec<_>=rows(&item["content"]).iter().map(|part|{
                match string(&part["type"]){
                    "image"=>{let url=string(&part["url"]);json!({"type":"image","image_url":{"url":url},"mimeType":url.strip_prefix("data:").and_then(|s|s.split_once(';')).map(|r|r.0).unwrap_or("image/png")})}
                    "text"=>upload_part(string(&part["text"])).unwrap_or_else(||part.clone()),
                    "skill"|"mention"=>json!({"type":"text","text":part["name"]}),
                    _=>json!({"type":"text","text":part["path"].as_str().unwrap_or("[attachment]")})
                }
            }).collect();
            Some(json!({"role":"user","id":id,"clientId":item["clientId"],"content":content}))
        }
        "agentMessage" => {
            let text = item["d4cText"]
                .as_str()
                .map_or_else(|| item["text"].clone(), |text| json!(text));
            let content = with_media(text, item_media(item));
            Some(json!({"role":"assistant","id":id,"content":content}))
        }
        "reasoning" => {
            let summary = lines(&item["summary"]);
            (!summary.is_empty())
                .then(|| json!({"role":"custom","display":true,"id":id,"content":summary}))
        }
        "plan" => Some(json!({"role":"custom","display":true,"id":id,"content":item["text"]})),
        _ => {
            let fields: &[&str] = match kind {
                "commandExecution" => &["command", "aggregatedOutput"],
                "fileChange" => &["changes"],
                "mcpToolCall" | "dynamicToolCall" => {
                    &["arguments", "error", "result", "contentItems"]
                }
                "functionCallOutput" => &["output"],
                "webSearch" => &["query"],
                "imageGeneration" => &["savedPath", "failure"],
                "imageView" => &["path"],
                "collabAgentToolCall" => &["prompt", "agentsStates"],
                _ => return None,
            };
            let attachments = item_media(item);
            let text = fields
                .iter()
                .filter(|key| {
                    attachments.is_empty() || !matches!(**key, "contentItems" | "savedPath")
                })
                .map(|key| &item[*key])
                .filter(|v| !v.is_null())
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect::<Vec<_>>()
                .join("\n");
            let content = with_media(json!(text), attachments);
            Some(
                json!({"role":"tool","id":id,"toolCallId":id,"toolName":item["tool"].as_str().or_else(||item["name"].as_str()).unwrap_or(kind),"isError":item["status"]=="failed" || item["success"]==false,"content":content}),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const THREAD: &str = "00000000-0000-4000-8000-000000000001";

    fn frames(adapter: &mut Adapter) -> Vec<Value> {
        std::mem::take(&mut adapter.output)
            .into_iter()
            .map(|o| match o {
                Output::Ui(v) | Output::Wire(v) => v,
            })
            .collect()
    }

    #[test]
    fn historical_workspace_and_schedule_prefixes_do_not_become_chat_titles() {
        let prompt = "[OpenClaw conversation info: sender={}]\nD4C scheduled tasks run in isolated sessions with delivery mode none. Preserve the originating owner.\n\nD4C workspace: no desktop is attached to this conversation. More instructions.\n\nFind my draft";
        assert_eq!(
            thread_title(&json!({"preview":prompt})).as_deref(),
            Some("Find my draft")
        );
    }
    #[test]
    fn imported_titles_skip_wrappers_and_recover_the_user_prompt() {
        let header =
            r#"[OpenClaw conversation info: sender={"name":"escaped ] delimiter","tags":["one"]}]"#;
        assert_eq!(
            thread_title(&json!({"name":header,"preview":format!("{header}\n\nRun the report")}))
                .as_deref(),
            Some("Run the report")
        );
        assert_eq!(
            thread_title(
                &json!({"preview":"D4C workspace: a persistent remote desktop is attached"})
            )
            .as_deref(),
            Some("Imported conversation")
        );
        assert_eq!(
            thread_title(&json!({"preview":"[OpenClaw conversation info: sender={" })).as_deref(),
            Some("Imported conversation")
        );
        assert_eq!(
            thread_title(&json!({"name":"My chat","preview":header})).as_deref(),
            Some("My chat")
        );
    }

    #[test]
    fn permissions_resume_without_overrides_and_update_only_the_selected_thread() {
        let mut adapter = Adapter::default();
        adapter.command(super::super::rpc(
            "sessions.messages.subscribe",
            json!({"key":THREAD}),
        ));
        let resume = frames(&mut adapter).pop().unwrap();
        assert_eq!(resume["method"], "thread/resume");
        assert!(resume["params"].get("sandbox").is_none());
        assert!(resume["params"].get("approvalPolicy").is_none());
        adapter.incoming(json!({"id":resume["id"],"result":{"thread":{"id":THREAD},"sandbox":{"type":"dangerFullAccess"},"approvalPolicy":"never"}}));
        let resumed = frames(&mut adapter);
        assert!(resumed.iter().any(|msg| msg["event"] == "session.settings"
            && msg["payload"]["sandbox"] == "dangerFullAccess"));
        for (mode, sandbox, approval) in [
            ("yolo", "dangerFullAccess", "never"),
            ("default", "workspaceWrite", "on-request"),
            ("read-only", "readOnly", "on-request"),
        ] {
            adapter.command(super::super::rpc(
                "permissions.set",
                json!({"key":THREAD,"mode":mode}),
            ));
            let update = frames(&mut adapter).pop().unwrap();
            assert_eq!(update["method"], "thread/settings/update");
            assert_eq!(
                update["params"],
                json!({"threadId":THREAD,"sandboxPolicy":{"type":sandbox},"approvalPolicy":approval})
            );
        }
    }

    #[test]
    fn stored_uploads_and_tool_images_are_visible_in_history_and_live_results() {
        let name = "clip from chat.mp4";
        let upload = format!("Attached file {name}: /data/uploads/{THREAD}/{name}");
        let row =
            transcript(&json!({"type":"userMessage","content":[{"type":"text","text":upload}]}))
                .unwrap();
        assert_eq!(
            row["content"][0]["artifactId"],
            format!("upload/{THREAD}/{name}")
        );
        assert!(upload_part(&format!("Attached file clip.mp4: /tmp/{THREAD}/clip.mp4")).is_none());
        let file = json!({"type":"image","artifactId":"blob/id/image.png","fileName":"image.png","mimeType":"image/png"});
        let item = json!({"id":"screen","type":"dynamicToolCall","tool":"screen","contentItems":[{"type":"inputText","text":"Saved","d4cAttachment":file}]});
        assert_eq!(transcript(&item).unwrap()["content"][0], file);
        let mut adapter = Adapter::default();
        adapter
            .incoming(json!({"method":"item/completed","params":{"item":item,"threadId":THREAD}}));
        assert_eq!(
            frames(&mut adapter)[0]["payload"]["data"]["result"]["content"][0],
            file
        );
        adapter.command(super::super::rpc(
            "artifacts.download",
            json!({"sessionKey":THREAD,"artifactId":file["artifactId"]}),
        ));
        assert_eq!(frames(&mut adapter)[0]["method"], "d4c/attachment/download");
    }

    #[test]
    fn completed_reply_video_is_available_in_live_chat_and_history() {
        let file = json!({"type":"file","artifactId":"blob/id/demo.mp4","fileName":"Demo.mp4","mimeType":"video/mp4"});
        let item = json!({"type":"agentMessage","id":"demo","text":"MEDIA: Demo.mp4","d4cText":"","d4cAttachments":[file]});
        let history = transcript(&item).unwrap();
        assert_eq!(history["content"], json!([file]));
        assert_eq!(crate::media::from_message(&history).len(), 1);

        let mut adapter = Adapter::default();
        adapter
            .incoming(json!({"method":"item/completed","params":{"item":item,"threadId":THREAD}}));
        let live = &frames(&mut adapter)[0]["payload"];
        assert_eq!(live["itemId"], "demo");
        assert_eq!(live["replace"], true);
        assert_eq!(live["message"], history);
    }

    #[test]
    fn empty_draft_is_local_until_first_send() {
        let mut a = Adapter::default();
        let p = json!({"sessionKey":"draft:scratch"});
        a.command(super::super::rpc("chat.history", p));
        assert_eq!(frames(&mut a)[0]["payload"]["messages"], json!([]));
        a.command(super::super::rpc(
            "chat.send",
            json!({"sessionKey":"draft:scratch","message":"hello","idempotencyKey":"client"}),
        ));
        let start = frames(&mut a).pop().unwrap();
        assert_eq!(start["method"], "thread/start");
        a.incoming(json!({"id":start["id"],"result":{"thread":{"id":THREAD}}}));
        let out = frames(&mut a);
        assert_eq!(out[0]["event"], "session.created");
        let send = out.iter().find(|o| o["method"] == "turn/start").unwrap();
        assert_eq!(send["params"]["threadId"], THREAD);
        assert_eq!(send["params"]["input"][0]["text"], "hello");
        assert_eq!(send["params"]["clientUserMessageId"], "client");
    }

    #[test]
    fn replayed_approval_stays_one_native_request_and_preserves_command_text() {
        let mut adapter = Adapter::default();
        let command = "echo `example`\necho '---'";
        let event = json!({"id":7,"method":"item/commandExecution/requestApproval","params":{
            "threadId":THREAD,"turnId":"turn","itemId":"item","command":command}});
        adapter.incoming(event.clone());
        adapter.incoming(event);
        let out = frames(&mut adapter);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0]["payload"]["questions"][0]["details"][0]["text"],
            command
        );
        assert_eq!(adapter.questions.len(), 1);
    }

    #[test]
    fn approval_requires_an_explicit_available_decision() {
        let mut a = Adapter::default();
        a.incoming(json!({"id":42,"method":"item/commandExecution/requestApproval","params":{"threadId":THREAD,"command":"pwd","availableDecisions":["decline"]}}));
        let out = frames(&mut a);
        assert_eq!(out.len(), 1);
        let q = &out[0]["payload"];
        assert_eq!(q["questions"][0]["options"].as_array().unwrap().len(), 1);
        for (choice, valid) in [("accept", false), ("decline", true)] {
            a.command(super::super::rpc(
                "question.resolve",
                json!({"id":q["id"],"answers":{"answers":{"decision":[choice]}}}),
            ));
            let out = frames(&mut a);
            assert_eq!(
                out.iter()
                    .any(|v| v["id"] == 42 && v["result"]["decision"] == "decline"),
                valid
            );
            assert_eq!(out.last().unwrap()["ok"], valid);
        }
    }

    #[test]
    fn active_thread_reload_stops_after_metadata_check() {
        let mut a = Adapter::default();
        a.command(super::super::rpc("sessions.reload", json!({"key":THREAD})));
        let read = frames(&mut a).pop().unwrap();
        assert_eq!(read["method"], "thread/read");
        a.incoming(json!({"id":read["id"],"result":{"thread":{"status":{"type":"active"}}}}));
        let out = frames(&mut a);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["ok"], false);
    }
}
