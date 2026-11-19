//! Native Codex dynamic tools for this conversation's desktop and scheduled tasks.
use crate::{manager::Manager, mcp};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs,
    io::{self, BufRead, BufReader, Write},
    path::Path,
    time::Duration,
};

fn namespace_spec() -> Value {
    let mut tools = mcp::tools().as_array().cloned().unwrap_or_default();
    tools.push(json!({"name":"desktop_context","description":"Check whether a desktop is attached to this conversation, its status, and who currently controls input.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}));
    tools.push(json!({
        "name":"automations",
        "description":"Manage this conversation's scheduled tasks. Jobs run in isolated Codex threads. For add, pass job with name, schedule, and payload {kind: agentTurn, message}. A schedule is {kind: every, everyMs}, {kind: at, at: ISO timestamp}, or {kind: cron, expr, tz}. For update, supply id and patch. Use runs and history to read previous results.",
        "inputSchema":{"type":"object","properties":{
            "action":{"type":"string","enum":["status","list","get","add","update","remove","run","runs","history"]},
            "id":{"type":"string"},"runId":{"type":"string"},"runAtMs":{"type":"integer"},
            "job":{"type":"object"},"patch":{"type":"object"},"includeDisabled":{"type":"boolean"},
            "limit":{"type":"integer","minimum":1},"offset":{"type":"integer","minimum":0}
        },"required":["action"],"additionalProperties":false}
    }));
    for tool in &mut tools {
        tool["type"] = json!("function");
    }
    json!({"type":"namespace","name":"d4c","description":"D4C tools scoped to this Codex conversation. Inspect the attached desktop with desktop_screenshot. Human takeover pauses agent input.","tools":tools})
}

fn owns_namespace(spec: &Value) -> bool {
    spec["type"] == "namespace"
        && matches!(
            spec["name"].as_str(),
            Some("d4c" | "openclaw" | "openclaw_direct")
        )
}

fn combine_specs(existing: &Value) -> Value {
    let mut specs = existing.as_array().cloned().unwrap_or_default();
    specs.retain(|s| !owns_namespace(s));
    specs.push(namespace_spec());
    Value::Array(specs)
}

pub fn augment_thread_start(request: &mut Value) {
    if request["method"] == "thread/start"
        && (request["params"].is_null() || request["params"].is_object())
    {
        let tools = combine_specs(&request["params"]["dynamicTools"]);
        request["params"]["dynamicTools"] = tools;
    }
}

fn text_result(text: impl Into<String>, success: bool) -> Value {
    json!({"success":success,"contentItems":[{"type":"inputText","text":text.into()}]})
}

fn execute(manager: &Manager, thread: &str, tool: &str, args: &Value) -> Result<Value, String> {
    if !args.is_object() {
        return Err("Tool arguments must be an object".into());
    }
    if tool == "automations" {
        let action = args["action"].as_str().ok_or("Task action is required")?;
        let mut p = if action == "add" {
            args["job"].clone()
        } else {
            args.clone()
        };
        if let Some(fields) = p.as_object_mut() {
            fields.remove("action");
        }
        return crate::schedules::for_thread(thread, action, &p)
            .map(|result| text_result(result.to_string(), true));
    }
    let method = if tool == "desktop_context" {
        "tool.context"
    } else {
        mcp::method(tool)?
    };
    let result = manager.agent(thread, method, args)?;
    let Some(content) = result["content"].as_array() else {
        return Ok(text_result(result.to_string(), true));
    };
    let mut items = vec![];
    for part in content {
        match part["type"].as_str() {
            Some("image") => {
                let data = part["data"]
                    .as_str()
                    .ok_or("Screenshot is missing image data")?;
                let mime = part["mimeType"].as_str().unwrap_or("image/png");
                items.push(
                    json!({"type":"inputImage","imageUrl":format!("data:{mime};base64,{data}")}),
                );
            }
            Some("text") => items.push(json!({"type":"inputText","text":part["text"]})),
            _ => return Err("Unsupported desktop result".into()),
        }
    }
    Ok(json!({"success":true,"contentItems":items}))
}

// A tool call may be redelivered after a disconnect or to multiple attached clients.
// Reserve the native call ID before issuing input or updating a schedule. A crash
// between action and result leaves an indeterminate receipt and cannot repeat the action.
fn execute_once(
    manager: &Manager,
    thread: &str,
    turn: &str,
    call: &str,
    tool: &str,
    args: &Value,
) -> Result<Value, String> {
    let mutating = match tool {
        "automations" => matches!(
            args["action"].as_str(),
            Some("run" | "add" | "update" | "remove")
        ),
        "desktop_screenshot" | "desktop_context" => false,
        _ => true,
    };
    if !mutating {
        return execute(manager, thread, tool, args);
    }
    let db = Connection::open(manager.runtime.root.join("codex-tool-calls.sqlite"))
        .map_err(|e| e.to_string())?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS tool_calls(thread TEXT NOT NULL,turn TEXT NOT NULL,call TEXT NOT NULL,result TEXT,PRIMARY KEY(thread,turn,call))").map_err(|e|e.to_string())?;
    let claimed = db
        .execute(
            "INSERT OR IGNORE INTO tool_calls(thread,turn,call) VALUES(?1,?2,?3)",
            params![thread, turn, call],
        )
        .map_err(|e| e.to_string())?;
    if claimed == 0 {
        let result: Option<String> = db
            .query_row(
                "SELECT result FROM tool_calls WHERE thread=?1 AND turn=?2 AND call=?3",
                params![thread, turn, call],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        return match result {
            Some(saved) => serde_json::from_str(&saved).map_err(|e| e.to_string()),
            None => {
                Err("This action was already dispatched. Its outcome is pending or unknown.".into())
            }
        };
    }
    let result = execute(manager, thread, tool, args).unwrap_or_else(|e| text_result(e, false));
    db.execute(
        "UPDATE tool_calls SET result=?4 WHERE thread=?1 AND turn=?2 AND call=?3",
        params![thread, turn, call, result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(result)
}

/// Only call this on a frame received from the authenticated upstream Codex socket.
pub fn answer(manager: &Manager, upstream_request: &Value) -> Option<Value> {
    let req = upstream_request;
    let p = &req["params"];
    if req["method"] != "item/tool/call" || req["id"].is_null() || p["namespace"] != "d4c" {
        return None;
    }
    let result = (|| {
        let thread = p["threadId"].as_str().ok_or("Missing thread id")?;
        uuid::Uuid::parse_str(thread).map_err(|_| "Invalid thread id")?;
        let turn = p["turnId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Missing turn id")?;
        let call = p["callId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Missing call id")?;
        let tool = p["tool"].as_str().ok_or("Missing tool name")?;
        execute_once(manager, thread, turn, call, tool, &p["arguments"])
    })()
    .unwrap_or_else(|e| text_result(e, false));
    Some(json!({"id":req["id"],"result":result}))
}

// Both activation and tool registration change byte offsets in the rollout. Codex
// rebuilds these three derived tables from the final files when the thread resumes.
fn invalidate_migrated_history(home: &Path, targets: &HashSet<String>) -> Result<(), String> {
    let path = home.join("thread_history_1.sqlite");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let mut database =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| e.to_string())?;
    let tx = database.transaction().map_err(|e| e.to_string())?;
    for table in [
        "thread_turns",
        "thread_items",
        "thread_history_projection_state",
    ] {
        let mut delete = tx
            .prepare(&format!("DELETE FROM {table} WHERE thread_id=?1"))
            .map_err(|e| e.to_string())?;
        for id in targets {
            delete.execute([id]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Run with the native server stopped, as the owner of the dedicated migrated Codex home.
/// Only the metadata line changes. Codex reads dynamic tools from this line on resume.
pub fn migrate_tools(home: &Path) -> Result<usize, String> {
    let manifest: Value = serde_json::from_reader(
        fs::File::open(home.join("migration-manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut targets: HashSet<String> = manifest["threads"]
        .as_object()
        .ok_or("Missing migration thread map")?
        .keys()
        .cloned()
        .collect();
    for row in manifest["sessions"]
        .as_object()
        .ok_or("Missing migration session map")?
        .values()
    {
        if let Some(id) = row["threadId"].as_str() {
            targets.insert(id.into());
        }
    }
    invalidate_migrated_history(home, &targets)?;
    let mut dirs = vec![home.join("sessions"), home.join("archived_sessions")];
    let mut changed = 0;
    while let Some(dir) = dirs.pop() {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                dirs.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let filename = entry.file_name();
            let Some(stem) = filename
                .to_str()
                .and_then(|s| s.strip_prefix("rollout-"))
                .and_then(|s| s.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let Some(id) = stem.len().checked_sub(36).and_then(|i| stem.get(i..)) else {
                continue;
            };
            if targets.remove(id) && migrate_rollout(&entry.path(), id)? {
                changed += 1;
            }
        }
    }
    if !targets.is_empty() {
        return Err(format!(
            "Missing {} rollout(s) referenced by migration manifest",
            targets.len()
        ));
    }
    Ok(changed)
}

fn migrate_rollout(path: &Path, id: &str) -> Result<bool, String> {
    let mut reader = BufReader::new(fs::File::open(path).map_err(|e| e.to_string())?);
    let mut line = Vec::new();
    reader
        .read_until(b'\n', &mut line)
        .map_err(|e| e.to_string())?;
    let mut first: Value = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
    if first["type"] != "session_meta" || first["payload"]["id"] != id {
        return Err(format!("Unexpected metadata for thread {id}"));
    }
    let existing = &first["payload"]["dynamic_tools"];
    if !existing.is_null() && !existing.is_array() {
        return Err(format!("Invalid dynamic tool metadata for thread {id}"));
    }
    let updated = combine_specs(existing);
    if *existing == updated {
        return Ok(false);
    }
    first["payload"]["dynamic_tools"] = updated;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)?;
        serde_json::to_writer(&mut writer, &first)?;
        writer.write_all(b"\n")?;
        io::copy(&mut reader, &mut writer)?;
        writer.sync_all()?;
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|_| true).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalidation_only_removes_derived_rows_for_migrated_threads() {
        let home = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&home).unwrap();
        let db = rusqlite::Connection::open(home.join("thread_history_1.sqlite")).unwrap();
        let tables = [
            "thread_turns",
            "thread_items",
            "thread_history_projection_state",
            "thread_realtime_items",
        ];
        for table in tables {
            db.execute_batch(&format!(
                "CREATE TABLE {table}(thread_id TEXT); INSERT INTO {table} VALUES ('old'), ('new');"
            ))
            .unwrap();
        }
        for _ in 0..2 {
            invalidate_migrated_history(&home, &HashSet::from(["old".into()])).unwrap();
        }
        for table in tables {
            let ids: String = db
                .query_row(
                    &format!("SELECT group_concat(thread_id) FROM {table}"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                ids,
                if table == "thread_realtime_items" {
                    "old,new"
                } else {
                    "new"
                }
            );
        }
        drop(db);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn registration_preserves_unrelated_tools_and_is_idempotent() {
        let other =
            json!({"type":"function","name":"my_tool","description":"unrelated","inputSchema":{}});
        let original = json!([other,{"type":"namespace","name":"openclaw_direct","tools":[]}]);
        let specs = combine_specs(&original);
        assert_eq!(specs[0], other);
        assert_eq!(specs[1]["name"], "d4c");
        assert_eq!(combine_specs(&specs), specs);
    }
    #[test]
    fn migration_changes_only_metadata_and_preserves_transcript_bytes() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        let tail = b"{ \"type\": \"turn_context\", \"payload\": {} }\n";
        let first = json!({"type":"session_meta","payload":{"id":"test"}});
        let mut data = first.to_string().into_bytes();
        data.push(b'\n');
        data.extend_from_slice(tail);
        fs::write(&path, data).unwrap();
        assert!(migrate_rollout(&path, "test").unwrap());
        let updated = fs::read(&path).unwrap();
        assert!(updated.ends_with(tail));
        assert!(!migrate_rollout(&path, "test").unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
