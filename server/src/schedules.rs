//! Scheduler RPCs share a transactional store with the timer worker.
use crate::store::Result;
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub fn rpc(action: &str, params: &Value) -> Result<Value> {
    invoke(None, action, params)
}

pub fn for_thread(thread_id: &str, action: &str, params: &Value) -> Result<Value> {
    uuid::Uuid::parse_str(thread_id).map_err(|_| "Missing native thread scope")?;
    invoke(Some(thread_id), action, params)
}

fn invoke(scope: Option<&str>, action: &str, params: &Value) -> Result<Value> {
    if !matches!(
        action,
        "status" | "list" | "get" | "add" | "update" | "remove" | "run" | "runs" | "history"
    ) {
        return Err("Unknown scheduler method".into());
    }
    let mut process = Command::new("/usr/local/libexec/d4c-scheduler")
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Scheduler unavailable: {e}"))?;
    let mut input = process.stdin.take().ok_or("No scheduler input")?;
    let written = input.write_all(
        &serde_json::to_vec(&json!({"action":action,"scope":scope,"params":params}))
            .map_err(|e| e.to_string())?,
    );
    drop(input);
    let output = process.wait_with_output().map_err(|e| e.to_string())?;
    written.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Scheduler helper failed".into());
    }
    let reply: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    if let Some(error) = reply["error"].as_str() {
        return Err(error.into());
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| "Invalid scheduler response".into())
}
