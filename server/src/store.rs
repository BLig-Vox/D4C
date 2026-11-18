use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub type Result<T> = std::result::Result<T, String>;
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path).map_err(|e| e.to_string())?;
        Self::init(Connection::open(path).map_err(|e| e.to_string())?)
    }
    fn init(db: Connection) -> Result<Self> {
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
   CREATE TABLE IF NOT EXISTS objects(kind TEXT NOT NULL,id TEXT NOT NULL,value TEXT NOT NULL,rev INTEGER NOT NULL,PRIMARY KEY(kind,id));
   CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,kind TEXT NOT NULL,id TEXT NOT NULL,value TEXT NOT NULL,created INTEGER NOT NULL);
   CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY,public_key TEXT NOT NULL,approved INTEGER NOT NULL DEFAULT 0);").map_err(|e|e.to_string())?;
        Ok(Self { db })
    }
    #[cfg(test)]
    pub fn memory() -> Self {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }
    pub fn get(&self, kind: &str, id: &str) -> Result<Value> {
        let row: Option<(String, i64)> = self
            .db
            .query_row(
                "SELECT value,rev FROM objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (text, rev) = row.ok_or_else(|| "not_found".to_string())?;
        let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        v["rev"] = json!(rev);
        Ok(v)
    }
    pub fn list(&self, kind: &str) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT value,rev FROM objects WHERE kind=?1 ORDER BY rev DESC LIMIT 2000")
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([kind], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            let (text, rev) = r.map_err(|e| e.to_string())?;
            let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            v["rev"] = json!(rev);
            Ok(v)
        })
        .collect()
    }
    pub fn revision(&self) -> Result<u64> {
        self.db
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|v| v as u64)
            .map_err(|e| e.to_string())
    }
    pub fn snapshot(&self) -> Result<Value> {
        let mut v = json!({"revision":self.revision()?});
        for kind in [
            "desktop",
            "binding",
            "draft",
            "preference",
            "notification",
            "activity",
        ] {
            v[kind] = json!(self.list(kind)?);
        }
        Ok(v)
    }
    pub fn changes(&self, since: u64) -> Result<Value> {
        let latest = self.revision()?;
        let oldest: i64 = self
            .db
            .query_row("SELECT COALESCE(MIN(seq),0) FROM events", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if since > latest || (since > 0 && oldest as u64 > since.saturating_add(1)) {
            return Ok(json!({"resync":true,"snapshot":self.snapshot()?}));
        }
        let mut q = self
            .db
            .prepare("SELECT seq,kind,id,value FROM events WHERE seq>?1 ORDER BY seq LIMIT 500")
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([since as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        let mut cursor = since;
        for row in rows {
            let (seq, kind, id, text) = row.map_err(|e| e.to_string())?;
            cursor = seq;
            let mut value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            value["rev"] = json!(seq);
            entries.push(json!({"seq":seq,"kind":kind,"id":id,"value":value}));
        }
        Ok(json!({"revision":cursor,"hasMore":cursor<latest,"events":entries}))
    }
    pub fn put(
        &mut self,
        kind: &str,
        id: &str,
        expected: Option<u64>,
        mut value: Value,
    ) -> Result<Value> {
        if !value.is_object() || value.to_string().len() > 1024 * 1024 || id.len() > 512 {
            return Err("invalid_object".into());
        }
        value.as_object_mut().unwrap().remove("rev");
        value["id"] = json!(id);
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        let current: i64 = tx
            .query_row(
                "SELECT rev FROM objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        if expected.is_some_and(|rev| rev != current as u64) {
            return Err(format!("conflict:{current}"));
        }
        let text = value.to_string();
        tx.execute(
            "INSERT INTO events(kind,id,value,created) VALUES(?1,?2,?3,?4)",
            params![kind, id, text, now() as i64],
        )
        .map_err(|e| e.to_string())?;
        let revision = tx.last_insert_rowid();
        tx.execute("INSERT INTO objects(kind,id,value,rev) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET value=excluded.value,rev=excluded.rev",params![kind,id,text,revision]).map_err(|e|e.to_string())?;
        tx.execute(
            "DELETE FROM events WHERE seq < ?1",
            [revision.saturating_sub(10_000)],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        value["rev"] = json!(revision);
        Ok(value)
    }
    pub fn delete(&mut self, kind: &str, id: &str, expected: u64) -> Result<Value> {
        let old = self.get(kind, id)?;
        if old["rev"].as_u64() != Some(expected) {
            return Err("conflict".into());
        }
        // A tombstone is durable and tells reconnecting clients to remove their cached row.
        self.put(
            kind,
            id,
            Some(expected),
            json!({"deleted":true,"name":old["name"]}),
        )
    }
    pub fn create_desktop(&mut self, name: &str) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 72 || name.chars().any(char::is_control) {
            return Err("invalid_name".into());
        }
        if self.list("desktop")?.iter().any(|d| {
            d["deleted"] != true
                && d["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase() == name.to_lowercase())
        }) {
            return Err("name_exists".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.put("desktop",&id,Some(0),json!({"name":name,"distro":"archlinux","desktop":"xfce","width":1280,"height":800,"memoryMb":1024,"cpus":1,"desired":"stopped","status":"new","mode":"agent","controller":null,"leaseUntil":0,"createdAt":now()}))
    }
    pub fn rename_desktop(&mut self, id: &str, name: &str, rev: u64) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 72 || name.chars().any(char::is_control) {
            return Err("invalid_name".into());
        }
        if self.list("desktop")?.iter().any(|d| {
            d["id"] != id
                && d["deleted"] != true
                && d["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        }) {
            return Err("name_exists".into());
        }
        let mut d = self.get("desktop", id)?;
        d["name"] = json!(name);
        self.put("desktop", id, Some(rev), d)
    }
    pub fn bind(&mut self, session: &str, id: Option<&str>, rev: u64) -> Result<Value> {
        if session.is_empty() || session.len() > 512 {
            return Err("invalid_session".into());
        }
        if let Some(id) = id {
            let d = self.get("desktop", id)?;
            if d["deleted"] == true {
                return Err("not_found".into());
            }
        }
        self.put(
            "binding",
            session,
            Some(rev),
            json!({"desktopId":id,"sessionKey":session}),
        )
    }
    pub fn claim(&mut self, id: &str, client: &str, time: u64) -> Result<()> {
        let mut d = self.get("desktop", id)?;
        if d["deleted"] == true {
            return Err("not_found".into());
        }
        if d["mode"] == "human"
            && d["leaseUntil"].as_u64().unwrap_or(0) > time
            && d["controller"].as_str() != Some(client)
        {
            return Err("controller_busy".into());
        }
        let rev = d["rev"].as_u64().unwrap();
        d["mode"] = json!("human");
        d["controller"] = json!(client);
        d["leaseUntil"] = json!(time.saturating_add(30_000));
        self.put("desktop", id, Some(rev), d)?;
        Ok(())
    }
    pub fn agent_input(&self, id: &str) -> Result<()> {
        let d = self.get("desktop", id)?;
        if d["deleted"] == true {
            return Err("not_found".into());
        }
        if d["mode"] != "agent" {
            return Err("human_takeover: agent input is paused".into());
        }
        Ok(())
    }
    pub fn human_input(&self, id: &str, client: &str, time: u64) -> Result<()> {
        let d = self.get("desktop", id)?;
        if d["deleted"] == true {
            return Err("not_found".into());
        }
        if d["mode"] != "human"
            || d["controller"].as_str() != Some(client)
            || d["leaseUntil"].as_u64().unwrap_or(0) <= time
        {
            return Err("controller_lease_required".into());
        }
        Ok(())
    }
    pub fn return_agent(&mut self, id: &str, client: &str, time: u64) -> Result<()> {
        let mut d = self.get("desktop", id)?;
        if d["mode"] == "human"
            && d["leaseUntil"].as_u64().unwrap_or(0) > time
            && d["controller"].as_str() != Some(client)
        {
            return Err("controller_busy".into());
        }
        let rev = d["rev"].as_u64().unwrap();
        d["mode"] = json!("agent");
        d["controller"] = Value::Null;
        d["leaseUntil"] = json!(0);
        self.put("desktop", id, Some(rev), d)?;
        Ok(())
    }
    pub fn request_device(&mut self, id: &str, key: &str) -> Result<bool> {
        let old: Option<(String, bool)> = self
            .db
            .query_row(
                "SELECT public_key,approved FROM devices WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((existing, approved)) = old {
            if existing != key {
                return Err("device_key_mismatch".into());
            }
            return Ok(approved);
        }
        let count: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if count >= 64 {
            return Err(
                "Device pairing limit reached; ask the server owner to remove an old device".into(),
            );
        }
        self.db
            .execute(
                "INSERT INTO devices(id,public_key,approved) VALUES(?1,?2,0)",
                params![id, key],
            )
            .map_err(|e| e.to_string())?;
        Ok(false)
    }
    pub fn approve_device(&mut self, id: &str) -> Result<()> {
        if self
            .db
            .execute("UPDATE devices SET approved=1 WHERE id=?1", [id])
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("unknown_device".into());
        }
        Ok(())
    }
    pub fn devices(&self) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT id,public_key,approved FROM devices")
            .map_err(|e| e.to_string())?;
        q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"publicKey":r.get::<_,String>(1)?,"approved":r.get::<_,bool>(2)?}))).map_err(|e|e.to_string())?.map(|r|r.map_err(|e|e.to_string())).collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_control_is_server_enforced_and_sticky() {
        let mut s = Store::memory();
        let d = s.create_desktop("Arch work").unwrap();
        let id = d["id"].as_str().unwrap();
        assert!(s.agent_input(id).is_ok());
        s.claim(id, "laptop", 1000).unwrap();
        assert!(s.agent_input(id).is_err());
        assert!(s.human_input(id, "laptop", 2000).is_ok());
        assert!(s.human_input(id, "phone", 2000).is_err());
        assert!(s.claim(id, "phone", 2000).is_err());
        assert!(s.human_input(id, "laptop", 40000).is_err());
        assert!(s.agent_input(id).is_err());
        s.claim(id, "phone", 40000).unwrap();
        s.return_agent(id, "phone", 41000).unwrap();
        assert!(s.agent_input(id).is_ok());
    }
    #[test]
    fn revisions_reject_stale_devices_and_resume_changes() {
        let mut s = Store::memory();
        let a = s
            .put("draft", "chat", Some(0), json!({"text":"hello"}))
            .unwrap();
        let rev = a["rev"].as_u64().unwrap();
        assert!(
            s.put("draft", "chat", Some(0), json!({"text":"stale"}))
                .is_err()
        );
        let b = s
            .put("draft", "chat", Some(rev), json!({"text":"new"}))
            .unwrap();
        assert_eq!(s.get("draft", "chat").unwrap()["text"], "new");
        assert_eq!(
            s.changes(rev).unwrap()["events"][0]["value"]["rev"],
            b["rev"]
        );
        assert_eq!(
            s.changes(rev).unwrap()["events"].as_array().unwrap().len(),
            1
        );
        assert!(
            s.changes(b["rev"].as_u64().unwrap() + 100).unwrap()["resync"]
                .as_bool()
                .unwrap()
        );
    }
    #[test]
    fn disk_reopen_preserves_desktop_draft_binding_and_pause() {
        let path = std::env::temp_dir().join(format!("d4c-store-{}.db", uuid::Uuid::new_v4()));
        let id;
        {
            let mut s = Store::open(&path).unwrap();
            let d = s.create_desktop("Persistent").unwrap();
            id = d["id"].as_str().unwrap().to_owned();
            s.bind("chat", Some(&id), 0).unwrap();
            s.claim(&id, "laptop", 1000).unwrap();
            s.put("draft", "chat", Some(0), json!({"text":"typing"}))
                .unwrap();
        }
        {
            let s = Store::open(&path).unwrap();
            assert_eq!(s.get("draft", "chat").unwrap()["text"], "typing");
            assert_eq!(s.get("binding", "chat").unwrap()["desktopId"], id);
            assert!(s.agent_input(&id).is_err());
        }
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn names_are_labels_not_directories_and_duplicates_are_rejected() {
        let mut s = Store::memory();
        let d = s.create_desktop("../../workspace").unwrap();
        assert!(uuid::Uuid::parse_str(d["id"].as_str().unwrap()).is_ok());
        assert!(s.create_desktop("../../WORKSPACE").is_err());
        assert!(s.create_desktop("\n").is_err());
    }
    #[test]
    fn device_keys_are_never_in_workspace_snapshots() {
        let mut s = Store::memory();
        assert!(!s.request_device("phone", "public-key").unwrap());
        s.approve_device("phone").unwrap();
        assert!(s.request_device("phone", "public-key").unwrap());
        assert!(s.request_device("phone", "other-key").is_err());
        assert!(!s.snapshot().unwrap().to_string().contains("public-key"));
    }
}
