use crate::{
    rfb,
    runtime::Runtime,
    store::{Result, Store, now},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
pub struct Manager {
    pub db: Arc<Mutex<Store>>,
    pub runtime: Arc<Runtime>,
    busy: Arc<Mutex<HashSet<String>>>,
}
#[derive(Clone, Debug)]
pub enum Actor {
    Human(String),
    Agent(String),
}
impl Manager {
    pub fn new(mut db: Store, runtime: Runtime) -> Self {
        if let Ok(desktops) = db.list("desktop") {
            for mut desktop in desktops.into_iter().filter(|d| d["deleted"] != true) {
                match runtime.remove_memory_limit(&desktop) {
                    Ok(()) => {
                        let Some(id) = desktop["id"].as_str().map(str::to_owned) else {
                            continue;
                        };
                        let revision = desktop["rev"].as_u64().unwrap_or(0);
                        if desktop
                            .as_object_mut()
                            .expect("Desktop object")
                            .remove("memoryMb")
                            .is_none()
                        {
                            continue;
                        }
                        if let Err(error) = db.put("desktop", &id, Some(revision), desktop) {
                            eprintln!("Desktop RAM limit removal persistence failed: {error}");
                        }
                    }
                    Err(error) => eprintln!("Desktop RAM limit removal failed: {error}"),
                }
            }
        }
        Self {
            db: Arc::new(Mutex::new(db)),
            runtime: Arc::new(runtime),
            busy: Arc::new(Mutex::new(HashSet::new())),
        }
    }
    pub fn dispatch(&self, actor: &Actor, method: &str, p: &Value) -> Result<Value> {
        if let Actor::Agent(session) = actor {
            return self.agent(session, method, p);
        }
        let Actor::Human(client) = actor else {
            unreachable!()
        };
        match method {
            "workspace.snapshot" => self
                .db
                .lock()
                .map_err(|_| "Database unavailable")?
                .snapshot(),
            "workspace.changes" => self
                .db
                .lock()
                .map_err(|_| "Database unavailable")?
                .changes(p["after"].as_u64().unwrap_or(0)),
            "workspace.rekey" => self
                .db
                .lock()
                .map_err(|_| "Database unavailable")?
                .rekey(field(p, "previousKey", 512)?, field(p, "sessionKey", 64)?),
            "workspace.put" => {
                let kind = field(p, "kind", 32)?;
                if !["draft", "preference"].contains(&kind) {
                    return Err("This collection is not client-writable".into());
                }
                let id = field(p, "id", 512)?;
                let rev = revision(p)?;
                self.db.lock().map_err(|_| "Database unavailable")?.put(
                    kind,
                    id,
                    Some(rev),
                    p["value"].clone(),
                )
            }
            "notification.import" => {
                let id = field(p, "id", 256)?;
                let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
                if let Ok(existing) = db.get("notification", id) {
                    return Ok(existing);
                }
                let mut value = p.clone();
                value["title"] = json!(
                    p["title"]
                        .as_str()
                        .unwrap_or("D4C")
                        .chars()
                        .take(120)
                        .collect::<String>()
                );
                value["body"] = json!(
                    p["body"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(16000)
                        .collect::<String>()
                );
                db.put("notification", id, Some(0), value)
            }
            "notification.read" => {
                let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
                let id = p["id"].as_str();
                let session = p["session"].as_str();
                for mut n in db.list("notification")? {
                    if n["unread"] == true
                        && id.is_none_or(|id| n["id"] == id)
                        && session.is_none_or(|s| n["session"] == s)
                    {
                        let id = n["id"].as_str().unwrap_or("").to_string();
                        let rev = n["rev"].as_u64().unwrap_or(0);
                        n["unread"] = json!(false);
                        db.put("notification", &id, Some(rev), n)?;
                    }
                }
                Ok(json!({"read":true}))
            }
            "desktop.list" => Ok(
                json!({"desktops":self.db.lock().map_err(|_|"Database unavailable")?.list("desktop")?}),
            ),
            "desktop.create" => {
                let name = field(p, "name", 256)?;
                let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
                if db
                    .list("desktop")?
                    .iter()
                    .filter(|d| d["deleted"] != true)
                    .count()
                    >= 16
                {
                    return Err("Desktop limit reached (16)".into());
                }
                let d = db.create_desktop(name)?;
                drop(db);
                self.launch(d, "prepare")
            }
            "desktop.rename" => self
                .db
                .lock()
                .map_err(|_| "Database unavailable")?
                .rename_desktop(field(p, "id", 64)?, field(p, "name", 256)?, revision(p)?),
            "desktop.start" | "desktop.stop" | "desktop.delete" => {
                let id = field(p, "id", 64)?;
                let db = self.db.lock().map_err(|_| "Database unavailable")?;
                let d = db.get("desktop", id)?;
                if d["deleted"] == true {
                    return Err("not_found".into());
                }
                if d["rev"].as_u64() != Some(revision(p)?) {
                    return Err("conflict".into());
                }
                if method == "desktop.start"
                    && db
                        .list("desktop")?
                        .iter()
                        .filter(|d| {
                            d["deleted"] != true && d["id"] != id && d["desired"] == "running"
                        })
                        .count()
                        >= 2
                {
                    return Err("Two desktops are already running. Stop one first.".into());
                }
                if method == "desktop.delete" && p["confirm"] != true {
                    return Err(
                        "Deletion confirmation required; persistent files will be retained".into(),
                    );
                }
                drop(db);
                self.launch(d, method.trim_start_matches("desktop."))
            }
            "desktop.claim" => {
                let id = field(p, "id", 64)?;
                let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
                let previous = db.get("desktop", id)?;
                db.claim(id, client, now())?;
                let out = db.get("desktop", id)?;
                drop(db);
                if previous["controller"].as_str() != Some(client.as_str())
                    || previous["mode"] != "human"
                {
                    self.runtime.reset_input(id)?;
                }
                Ok(out)
            }
            "desktop.return_agent" => {
                let id = field(p, "id", 64)?;
                let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
                db.return_agent(id, client, now())?;
                let out = db.get("desktop", id)?;
                drop(db);
                self.runtime.reset_input(id)?;
                Ok(out)
            }
            "desktop.bind" => {
                let session = field(p, "session", 512)?;
                let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
                if db
                    .get("activity", session)
                    .is_ok_and(|v| v["running"] == true)
                {
                    return Err(
                        "A run is active in this chat. Wait or stop it before changing desktops."
                            .into(),
                    );
                }
                let desktop = if p["name"].is_null() && p["desktopId"].is_null() {
                    None
                } else {
                    let name = p["name"].as_str().unwrap_or("").to_lowercase();
                    let id = p["desktopId"].as_str().unwrap_or("");
                    Some(
                        db.list("desktop")?
                            .into_iter()
                            .find(|d| {
                                d["deleted"] != true
                                    && (d["id"] == id
                                        || d["name"]
                                            .as_str()
                                            .is_some_and(|n| n.to_lowercase() == name))
                            })
                            .ok_or("Desktop not found")?,
                    )
                };
                db.bind(
                    session,
                    desktop.as_ref().and_then(|d| d["id"].as_str()),
                    revision(p)?,
                )
            }
            "desktop.input" => {
                let id = field(p, "id", 64)?;
                let bytes = input(p)?;
                self.runtime.input(&self.db, id, Some(client), &bytes)?;
                Ok(json!({"sent":true}))
            }
            _ => Err("Unknown method".into()),
        }
    }
    pub fn agent(&self, session: &str, method: &str, p: &Value) -> Result<Value> {
        if ![
            "tool.context",
            "tool.screenshot",
            "tool.click",
            "tool.move",
            "tool.scroll",
            "tool.type",
            "tool.key",
        ]
        .contains(&method)
        {
            return Err("Agent transport cannot manage workspace or control authority".into());
        }
        let db = self.db.lock().map_err(|_| "Database unavailable")?;
        if method == "tool.context" {
            let desktop = db
                .get("binding", session)
                .ok()
                .and_then(|b| b["desktopId"].as_str().map(str::to_owned))
                .and_then(|id| db.get("desktop", &id).ok())
                .filter(|d| d["deleted"] != true);
            return Ok(match desktop {
                Some(d) => {
                    json!({"attached":true,"desktop":{"id":d["id"],"name":d["name"],"status":d["status"],"mode":d["mode"]}})
                }
                None => json!({"attached":false}),
            });
        }
        let binding = db
            .get("binding", session)
            .map_err(|_| "No desktop attached to this conversation")?;
        let id = binding["desktopId"]
            .as_str()
            .ok_or("No desktop attached to this conversation")?
            .to_string();
        let desktop = db.get("desktop", &id)?;
        if desktop["deleted"] == true {
            return Err("Desktop was removed".into());
        }
        drop(db);
        if method == "tool.screenshot" {
            use base64::Engine as _;
            let png = self.runtime.screenshot(&id)?;
            return Ok(
                json!({"content":[{"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(png)}]}),
            );
        }
        let bytes = match method {
            "tool.type" => rfb::text_events(field(p, "text", 16384)?),
            "tool.key" => {
                let combo = field(p, "key", 128)?;
                let mut b = Vec::new();
                let keys: Vec<_> = combo
                    .split('+')
                    .map(|s| rfb::named_key(s).ok_or_else(|| format!("Unknown key {s}")))
                    .collect::<Result<_>>()?;
                for k in &keys {
                    b.extend(rfb::key_event(*k, true));
                }
                for k in keys.iter().rev() {
                    b.extend(rfb::key_event(*k, false));
                }
                b
            }
            "tool.move" => pointer(p, 0)?.to_vec(),
            "tool.click" => {
                let mask = match p["button"].as_u64().unwrap_or(1) {
                    1 => 1,
                    2 => 2,
                    3 => 4,
                    _ => return Err("Invalid mouse button".into()),
                };
                let mut b = pointer(p, mask)?.to_vec();
                b.extend(pointer(p, 0)?);
                b
            }
            "tool.scroll" => {
                let mask = match p["direction"].as_str() {
                    Some("up") => 8,
                    Some("down") => 16,
                    _ => return Err("Direction must be up or down".into()),
                };
                let steps = p["steps"].as_u64().unwrap_or(3).clamp(1, 12);
                let mut b = Vec::new();
                for _ in 0..steps {
                    b.extend(pointer(p, mask)?);
                    b.extend(pointer(p, 0)?);
                }
                b
            }
            _ => return Err("Unknown tool".into()),
        };
        self.runtime.input(&self.db, &id, None, &bytes)?;
        Ok(json!({"content":[{"type":"text","text":"Done"}]}))
    }
    fn launch(&self, mut d: Value, action: &str) -> Result<Value> {
        let id = d["id"].as_str().ok_or("Desktop ID missing")?.to_string();
        if !self
            .busy
            .lock()
            .map_err(|_| "Lifecycle lock unavailable")?
            .insert(id.clone())
        {
            return Err("Desktop operation already in progress".into());
        }
        let rev = d["rev"].as_u64().unwrap_or(0);
        d["status"] = json!(match action {
            "start" => "starting",
            "stop" => "stopping",
            "delete" => "deleting",
            _ => "creating",
        });
        if action == "start" {
            d["desired"] = json!("running");
        } else if action != "prepare" {
            d["desired"] = json!("stopped");
        }
        d["lastError"] = Value::Null;
        let saved = (|| {
            let mut db = self.db.lock().map_err(|_| "Database unavailable")?;
            if action == "start"
                && db
                    .list("desktop")?
                    .iter()
                    .filter(|other| {
                        other["deleted"] != true
                            && other["id"] != id
                            && other["desired"] == "running"
                    })
                    .count()
                    >= 2
            {
                return Err("Two desktops are already running. Stop one first.".into());
            }
            db.put("desktop", &id, Some(rev), d)
        })();
        let saved = match saved {
            Ok(v) => v,
            Err(e) => {
                self.busy
                    .lock()
                    .map_err(|_| "Lifecycle lock unavailable")?
                    .remove(&id);
                return Err(e);
            }
        };
        let manager = self.clone();
        let action = action.to_string();
        let out = saved.clone();
        std::thread::spawn(move || {
            let result = (|| match action.as_str() {
                "start" => {
                    manager.runtime.prepare(&saved)?;
                    manager.runtime.start(&id)
                }
                "stop" => {
                    manager.runtime.prepare(&saved)?;
                    manager.runtime.stop(&id)
                }
                "delete" => manager.runtime.remove(&id),
                _ => manager.runtime.prepare(&saved),
            })();
            if let Ok(mut db) = manager.db.lock() {
                if let Ok(mut current) = db.get("desktop", &id) {
                    let revision = current["rev"].as_u64().unwrap_or(0);
                    if result.is_ok() && action == "delete" {
                        let _ = db.delete("desktop", &id, revision);
                    } else {
                        current["status"] = json!(if result.is_err() {
                            "error"
                        } else if action == "start" {
                            "running"
                        } else {
                            "stopped"
                        });
                        current["lastError"] =
                            result.err().map(Value::String).unwrap_or(Value::Null);
                        let _ = db.put("desktop", &id, Some(revision), current);
                    }
                }
            }
            if let Ok(mut busy) = manager.busy.lock() {
                busy.remove(&id);
            }
        });
        Ok(out)
    }
    pub fn reconcile(&self) {
        let jobs = self
            .db
            .lock()
            .ok()
            .and_then(|d| d.list("desktop").ok())
            .unwrap_or_default();
        for d in jobs.into_iter().filter(|d| d["deleted"] != true) {
            let action = if d["status"] == "deleting" {
                "delete"
            } else if d["desired"] == "running" {
                "start"
            } else {
                "stop"
            };
            let _ = self.launch(d, action);
        }
    }
}
fn field<'a>(p: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    let s = p[key].as_str().ok_or_else(|| format!("{key} required"))?;
    if s.is_empty() || s.len() > max {
        return Err(format!("Invalid {key}"));
    }
    Ok(s)
}
fn revision(p: &Value) -> Result<u64> {
    p["expectedRevision"]
        .as_u64()
        .ok_or("expectedRevision required".into())
}
fn pointer(p: &Value, mask: u8) -> Result<[u8; 6]> {
    let x = p["x"]
        .as_u64()
        .filter(|n| *n < rfb::MAX_WIDTH as u64)
        .ok_or("Invalid x coordinate")?;
    let y = p["y"]
        .as_u64()
        .filter(|n| *n < rfb::MAX_HEIGHT as u64)
        .ok_or("Invalid y coordinate")?;
    Ok(rfb::pointer_event(x as u16, y as u16, mask))
}
fn input(p: &Value) -> Result<Vec<u8>> {
    match p["type"].as_str().unwrap_or("") {
        "key" => {
            let key = p["key"]
                .as_u64()
                .filter(|n| *n <= u32::MAX as u64)
                .ok_or("Invalid key")?;
            Ok(rfb::key_event(key as u32, p["down"].as_bool().unwrap_or(false)).to_vec())
        }
        "pointer" => Ok(pointer(
            p,
            p["buttons"]
                .as_u64()
                .filter(|n| *n <= 31)
                .ok_or("Invalid button state")? as u8,
        )?
        .to_vec()),
        "text" => Ok(rfb::text_events(field(p, "text", 16384)?)),
        "batch" => {
            let values = p["events"]
                .as_array()
                .filter(|v| v.len() <= 64)
                .ok_or("Invalid input batch")?;
            let mut bytes = Vec::new();
            for v in values {
                if v["type"] == "batch" {
                    return Err("Nested batches are not allowed".into());
                }
                bytes.extend(input(v)?);
                if bytes.len() > 131072 {
                    return Err("Input batch too large".into());
                }
            }
            Ok(bytes)
        }
        _ => Err("Unknown input type".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_is_typed_and_nested_batches_are_rejected() {
        assert!(input(&json!({"type":"pointer","x":99999,"y":0,"buttons":0})).is_err());
        assert!(input(&json!({"type":"batch","events":[{"type":"batch","events":[]}]})).is_err());
        assert_eq!(
            input(&json!({"type":"key","key":65,"down":true})).unwrap(),
            rfb::key_event(65, true)
        );
    }
}
