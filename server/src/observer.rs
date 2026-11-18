//! Trusted OpenClaw activity ingress; never exposed through the network RPC dispatcher.
use crate::{
    manager::Manager,
    store::{Result, now},
};
use serde_json::{Value, json};
pub fn record(manager: &Manager, session: &str, p: &Value) -> Result<Value> {
    if session.is_empty() || session.len() > 512 {
        return Err("Invalid session".into());
    }
    let run = p["runId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or("runId required")?;
    let phase = p["phase"].as_str().unwrap_or("running");
    if !["running", "finished", "error", "aborted"].contains(&phase) {
        return Err("Invalid phase".into());
    }
    let time = p["time"].as_u64().unwrap_or_else(now);
    let mut db = manager.db.lock().map_err(|_| "Database unavailable")?;
    if p["kind"] == "cron" {
        let job = p["job"].as_str().filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or("Cron job required")?;
        let id = format!("cron:{job}:{run}");
        let text: String = p["text"].as_str().unwrap_or("").chars().take(16000).collect();
        if let Ok(mut existing) = db.get("notification", &id) {
            let rev = existing["rev"].as_u64().unwrap_or(0);
            existing["session"] = json!(session);
            existing["body"] = json!(text);
            existing["status"] = p["status"].clone();
            db.put("notification", &id, Some(rev), existing)?;
        } else {
            let title: String = p["title"].as_str().unwrap_or("Scheduled task").chars().take(120).collect();
            db.put("notification", &id, Some(0), json!({"session":session,"runId":run,
                "job":job,"runAtMs":p["runAtMs"],"kind":"cron","title":title,
                "body":text,"unread":true,"time":time,"status":p["status"]}))?;
        }
        return Ok(json!({"recorded":true}));
    }
    let old = db.get("activity", session).ok();
    // Do not let retried old frames overwrite a newer run or revive a completed one.
    let stale = old.as_ref().is_some_and(|o| {
        o["time"].as_u64().unwrap_or(0) > time
            || (o["runId"] == run && o["running"] == false && phase == "running")
    });
    let rev = old.as_ref().and_then(|v| v["rev"].as_u64()).unwrap_or(0);
    let text = p["text"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(16000)
        .collect::<String>();
    let terminal = phase != "running";
    if !stale {
        db.put("activity",session,Some(rev),json!({"runId":run,"running":!terminal,"phase":phase,"time":time,"text":text,"label":p["label"].as_str().unwrap_or("Working").chars().take(120).collect::<String>()}))?;
    }
    if terminal {
        let id = format!("chat:{run}");
        if let Ok(mut existing) = db.get("notification", &id) {
            if !text.is_empty() && existing["body"] != text {
                let revision = existing["rev"].as_u64().unwrap_or(0);
                existing["body"] = json!(text);
                existing["status"] = json!(phase);
                db.put("notification", &id, Some(revision), existing)?;
            }
        } else {
            db.put("notification",&id,Some(0),json!({"session":session,"run":run,"kind":"chat","title":if phase=="finished"{"Reply ready"}else{"Run ended"},"body":text,"unread":true,"time":time,"status":phase}))?;
        }
    }
    Ok(json!({"recorded":true}))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{runtime::Runtime, store::Store};
    #[test]
    fn terminal_events_are_durable_and_not_reopened_by_retries() {
        let root = std::env::temp_dir().join(format!("d4c-observer-{}", uuid::Uuid::new_v4()));
        let manager = Manager::new(
            Store::open(&root.join("db")).unwrap(),
            Runtime::new(root.clone()).unwrap(),
        );
        record(
            &manager,
            "chat",
            &json!({"runId":"r","phase":"running","time":1}),
        )
        .unwrap();
        record(
            &manager,
            "chat",
            &json!({"runId":"r","phase":"finished","time":3,"text":"Done"}),
        )
        .unwrap();
        record(
            &manager,
            "chat",
            &json!({"runId":"r","phase":"running","time":2}),
        )
        .unwrap();
        record(
            &manager,
            "chat",
            &json!({"runId":"older","phase":"finished","time":1,"text":"Older completion"}),
        )
        .unwrap();
        let db = manager.db.lock().unwrap();
        assert_eq!(db.get("activity", "chat").unwrap()["running"], false);
        assert_eq!(db.list("notification").unwrap().len(), 2);
        assert_eq!(db.get("activity", "chat").unwrap()["runId"], "r");
        drop(db);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
}
