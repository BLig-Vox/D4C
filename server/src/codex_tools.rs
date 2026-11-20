//! Native Codex dynamic tools for this conversation's desktop and scheduled tasks.
use crate::{manager::Manager, mcp};
use base64::{Engine, engine::general_purpose::STANDARD};
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
    tools.push(json!({"name":"share_media","description":"Send an image or video in this chat. First call without arguments to get this chat’s export directory. Save or copy a PNG, JPEG, GIF, WebP, MP4, MKV, MOV or WebM there (up to 6 MB), then call with its filename.","inputSchema":{"type":"object","properties":{"filename":{"type":"string"}},"additionalProperties":false}}));
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
    if tool == "share_media" {
        return crate::chat_media::share(&manager.runtime.root, thread, args["filename"].as_str())
            .map(|v| text_result(v.to_string(), true));
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
                let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
                let stored = crate::transport::store_attachment(
                    &manager.runtime.root.join("uploads"),
                    &json!({"name":"Screenshot.png","data":data}),
                )?;
                let path = Path::new(stored["path"].as_str().ok_or("Missing screenshot path")?);
                let key = path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|v| v.to_str())
                    .ok_or("Invalid screenshot path")?;
                let original = json!({"type":"image","artifactId":format!("upload/{key}/Screenshot.png"),"fileName":"Screenshot.png","mimeType":"image/png"});
                let frame = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                let mut preview = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut preview, 70)
                    .encode_image(&frame.to_rgb8())
                    .map_err(|e| e.to_string())?;
                let view_path = if preview.len() < bytes.len() {
                    let preview_path = path.with_file_name("Preview.jpg");
                    fs::write(&preview_path, preview).map_err(|e| e.to_string())?;
                    preview_path
                } else {
                    path.to_owned()
                };
                items.push(json!({"type":"inputText","text":json!({
                    "d4cScreenshot":original,
                    "path":view_path,
                    "instruction":"Inspect this capture with native view_image using path. In Code Mode, forward its image_url with image()."
                }).to_string()}));
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
    if req["method"] != "item/tool/call"
        || req["id"].is_null()
        || !matches!(
            p["namespace"].as_str(),
            Some("d4c" | "openclaw" | "openclaw_direct")
        )
    {
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
        if p["namespace"] != "d4c" && tool != "desktop_context" && mcp::method(tool).is_err() {
            return Err(format!("Legacy tool {tool} is unavailable. Use native Codex skills and the d4c desktop tools."));
        }
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
/// Resumes can recover dynamic tools from the header or a compaction checkpoint.
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
    visit_rollouts(home, |path, id| {
        if targets.remove(id) {
            migrate_rollout(path, id, None)
        } else {
            Ok(false)
        }
    })
    .and_then(|count| {
        if targets.is_empty() {
            Ok(count)
        } else {
            Err(format!(
                "Missing {} rollout(s) referenced by migration manifest",
                targets.len()
            ))
        }
    })
}

/// ExecStartPre runs before Codex opens any rollout file descriptors.
pub fn refresh_tools(home: &Path) -> Result<usize, String> {
    let marker = home.join(".d4c-verified-tool-catalog");
    let catalog = namespace_spec().to_string();
    let verify_checkpoints = fs::read_to_string(&marker).ok().as_ref() != Some(&catalog);
    let count = visit_rollouts(home, |path, id| {
        migrate_rollout(path, id, Some((home, verify_checkpoints)))
    })?;
    if verify_checkpoints {
        let temp = marker.with_extension(uuid::Uuid::new_v4().to_string());
        fs::write(&temp, catalog)
            .and_then(|_| fs::rename(&temp, marker))
            .map_err(|e| e.to_string())?;
    }
    Ok(count)
}

fn visit_rollouts(
    home: &Path,
    mut visit: impl FnMut(&Path, &str) -> Result<bool, String>,
) -> Result<usize, String> {
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
            if visit(&entry.path(), id)? {
                changed += 1;
            }
        }
    }
    Ok(changed)
}

fn migrate_rollout(path: &Path, id: &str, refresh: Option<(&Path, bool)>) -> Result<bool, String> {
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
    if refresh.is_some()
        && !existing.as_array().is_some_and(|specs| {
            specs
                .iter()
                .any(|s| s["type"] == "namespace" && s["name"] == "d4c")
        })
    {
        return Ok(false);
    }
    let updated = combine_specs(existing);
    if matches!(refresh, Some((_, false))) && existing == &updated {
        return Ok(false);
    }
    let mut changed = *existing != updated;
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
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            let mut event: Value = serde_json::from_slice(&line)?;
            if event["type"] == "compacted"
                && let Some(tools) =
                    event.pointer_mut("/payload/resume_metadata/dynamic_tools/tools")
                && tools.is_array()
            {
                let next = combine_specs(tools);
                if *tools != next {
                    *tools = next;
                    serde_json::to_writer(&mut writer, &event)?;
                    writer.write_all(b"\n")?;
                    changed = true;
                    continue;
                }
            }
            writer.write_all(&line)?;
        }
        if !changed {
            drop(writer);
            return fs::remove_file(&temp);
        }
        writer.sync_all()?;
        if let Some((home, _)) = refresh {
            invalidate_migrated_history(home, &HashSet::from([id.to_owned()]))
                .map_err(io::Error::other)?;
        }
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|_| changed).map_err(|e| e.to_string())
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
    fn startup_repairs_old_checkpoints_and_skips_unmanaged_chats() {
        let home = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let dir = home.join("sessions");
        fs::create_dir_all(&dir).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = dir.join(format!("rollout-{id}.jsonl"));
        let header =
            json!({"type":"session_meta","payload":{"id":id,"dynamic_tools":[namespace_spec()]}});
        let checkpoint = json!({"type":"compacted","payload":{"resume_metadata":{"dynamic_tools":{"tools":[{"type":"namespace","name":"d4c","tools":[]}]}}}});
        fs::write(&path, format!("{header}\n{checkpoint}\n")).unwrap();
        let other_id = uuid::Uuid::new_v4().to_string();
        let other = dir.join(format!("rollout-{other_id}.jsonl"));
        let unmanaged = json!({"type":"session_meta","payload":{"id":other_id}}).to_string();
        fs::write(&other, &unmanaged).unwrap();
        assert_eq!(refresh_tools(&home).unwrap(), 1);
        assert_eq!(fs::read_to_string(other).unwrap(), unmanaged);
        let data = fs::read_to_string(&path).unwrap();
        assert!(data.lines().nth(1).unwrap().contains("share_media"));
        assert_eq!(refresh_tools(&home).unwrap(), 0);
        fs::remove_dir_all(home).unwrap();
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
        let checkpoint = json!({"type":"compacted","payload":{"resume_metadata":{"dynamic_tools":{"thread_id":"test","tools":[{"type":"namespace","name":"openclaw","tools":[]}]}}}});
        data.extend_from_slice(checkpoint.to_string().as_bytes());
        data.push(b'\n');
        data.extend_from_slice(tail);
        fs::write(&path, data).unwrap();
        assert!(migrate_rollout(&path, "test", None).unwrap());
        let updated = fs::read(&path).unwrap();
        assert!(updated.ends_with(tail));
        let checkpoint: Value =
            serde_json::from_slice(updated.split(|b| *b == b'\n').nth(1).unwrap()).unwrap();
        assert_eq!(
            checkpoint["payload"]["resume_metadata"]["dynamic_tools"]["tools"][0]["name"],
            "d4c"
        );
        assert!(!migrate_rollout(&path, "test", None).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
