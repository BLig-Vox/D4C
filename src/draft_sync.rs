use serde_json::{Value, json};
use std::time::{Duration, Instant};
#[derive(Default)]
pub struct Queue {
    rows: std::collections::HashMap<String, Row>,
    device: Option<String>,
}
struct Row {
    base: String,
    local: String,
    revision: u64,
    pending: Option<(String, Instant)>,
    pub conflict: bool,
}
impl Queue {
    pub fn set_device(&mut self, device: Option<String>) {
        self.device = device;
    }
    pub fn restore(&mut self, id: &str, local: &str, base: &str, revision: u64) {
        self.rows.insert(
            id.into(),
            Row {
                base: base.into(),
                local: local.into(),
                revision,
                pending: None,
                conflict: false,
            },
        );
    }
    pub fn forget(&mut self, id: &str) {
        self.rows.remove(id);
    }
    pub fn synchronize(&mut self, id: &str, local: &str, remote: &Value) -> String {
        let text = remote["text"].as_str().unwrap_or("");
        let revision = remote["rev"].as_u64().unwrap_or(0);
        let initial = !self.rows.contains_key(id);
        let row = self.rows.entry(id.into()).or_insert_with(|| Row {
            base: text.into(),
            local: local.into(),
            revision,
            pending: None,
            conflict: false,
        });
        row.local = if initial && local.is_empty() {
            text.into()
        } else {
            local.into()
        };
        // Successful writes can be acknowledged after the retry window, or after
        // further typing. A save from this physical device is not a foreign edit.
        if revision < row.revision {
            return row.local.clone();
        }
        let own_write = self
            .device
            .as_deref()
            .is_some_and(|device| remote["writerDevice"].as_str() == Some(device));
        let echoed_write = row.pending.as_ref().is_some_and(|(value, _)| value == text);
        if row.local == text || echoed_write || (revision != row.revision && own_write) {
            row.base = text.into();
            row.pending = None;
            row.conflict = false;
        } else if revision != row.revision {
            if row.local == row.base && row.pending.is_none() {
                row.local = text.into();
                row.base = text.into();
                row.conflict = false;
            } else if text != row.base {
                row.conflict = true;
            }
        }
        row.revision = revision;
        row.local.clone()
    }
    pub fn requests(&mut self) -> Vec<Value> {
        let mut out = vec![];
        for (id, row) in self.rows.iter_mut() {
            if row
                .pending
                .as_ref()
                .is_some_and(|(_, time)| time.elapsed() > Duration::from_secs(5))
            {
                row.pending = None;
            }
            if row.local != row.base
                && !row.conflict
                && row.pending.is_none()
                && row.local.len() < 256 * 1024
            {
                out.push(json!({"kind":"draft","id":id,"expectedRevision":row.revision,"value":{"text":row.local,"writerDevice":self.device}}));
                row.pending = Some((row.local.clone(), Instant::now()));
            }
        }
        out
    }
    pub fn conflicted(&self, id: &str) -> bool {
        self.rows.get(id).is_some_and(|r| r.conflict)
    }
    pub fn pending(&self) -> bool {
        self.rows
            .values()
            .any(|r| r.pending.is_some() || r.local != r.base)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleted_draft_is_not_reuploaded() {
        let mut q = Queue::default();
        q.synchronize("gone", "sensitive draft", &json!({"rev":0,"text":""}));
        q.forget("gone");
        assert!(q.requests().is_empty());
        assert!(!q.pending());
    }
    #[test]
    fn outgoing_chat_remains_in_queue_until_latest_edit_is_acknowledged() {
        let mut q = Queue::default();
        q.synchronize("a", "first", &json!({"rev":0,"text":""}));
        assert_eq!(q.requests().len(), 1);
        q.synchronize("a", "latest", &json!({"rev":0,"text":""}));
        q.synchronize("b", "", &json!({"rev":0,"text":""}));
        q.synchronize("a", "latest", &json!({"rev":1,"text":"first"}));
        let request = q.requests();
        assert_eq!(request[0]["id"], "a");
        assert_eq!(request[0]["value"]["text"], "latest");
        assert_eq!(request[0]["expectedRevision"], 1);
        q.synchronize("a", "latest", &json!({"rev":2,"text":"latest"}));
        assert!(!q.pending());
    }
    #[test]
    fn remote_edit_does_not_overwrite_dirty_local_text() {
        let mut q = Queue::default();
        q.synchronize("a", "", &json!({"rev":1,"text":""}));
        assert_eq!(
            q.synchronize("a", "mine", &json!({"rev":2,"text":"theirs"})),
            "mine"
        );
        assert!(q.conflicted("a"));
        assert!(q.requests().is_empty());
    }
}
