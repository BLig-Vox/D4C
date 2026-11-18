//! Native Android picker bridge. URI reads happen on Java's worker thread;
//! Rust validates private temporary paths and delivers bounded attachment bytes.
use crate::uploads::{FILE_LIMIT, TOTAL_LIMIT, UploadResult};
use eframe::egui;
use jni::{
    JValue, jni_sig, jni_str,
    objects::{JObject, JString},
    refs::Global,
};
use std::{
    io::Read,
    sync::{Mutex, mpsc},
};

struct Pending {
    id: String,
    tx: mpsc::Sender<UploadResult>,
    ctx: egui::Context,
}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

pub fn cancel() {
    if let Ok(mut slot) = PENDING.lock() {
        *slot = None;
    }
}
fn finish(id: &str, result: UploadResult) {
    let pending = PENDING.lock().ok().and_then(|mut slot| {
        if slot.as_ref().is_some_and(|p| p.id == id) {
            slot.take()
        } else {
            None
        }
    });
    if let Some(pending) = pending {
        let _ = pending.tx.send(result);
        pending.ctx.request_repaint();
    }
}
pub fn pick(ctx: egui::Context) -> mpsc::Receiver<UploadResult> {
    let (tx, rx) = mpsc::channel();
    let Some(app) = crate::platform::android_app() else {
        let _ = tx.send(Err("Android activity unavailable".into()));
        return rx;
    };
    let id = uuid::Uuid::new_v4().to_string();
    {
        let Ok(mut slot) = PENDING.lock() else {
            let _ = tx.send(Err("File picker unavailable".into()));
            return rx;
        };
        if slot.is_some() {
            let _ = tx.send(Err("A file picker is already open".into()));
            return rx;
        }
        *slot = Some(Pending {
            id: id.clone(),
            tx,
            ctx,
        });
    }
    let activity_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        // AndroidApp owns these VM/activity references for the duration of this callback.
        let vm = unsafe { jni::JavaVM::from_raw(activity_app.vm_as_ptr().cast()) };
        let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let raw = activity_app.activity_as_ptr() as jni::sys::jobject;
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
            let token = env.new_string(&id)?;
            env.call_method(
                activity.as_ref(),
                jni_str!("pickFiles"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&token)],
            )?;
            Ok(())
        });
        if let Err(error) = result {
            finish(&id, Err(format!("Could not open Android files: {error}")));
        }
    }));
    rx
}
fn read_result(json: &str) -> UploadResult {
    if json.len() > 32_768 {
        return Err("File picker response too large".into());
    }
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.into());
    }
    let rows = value["files"]
        .as_array()
        .ok_or("Invalid file picker response")?;
    if rows.is_empty() {
        return Ok(vec![]);
    }
    if rows.len() > 8 {
        return Err("Choose up to 8 files at a time".into());
    }
    let root = crate::platform::attachment_cache()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut files = vec![];
    let mut total = 0;
    for row in rows {
        let path = std::path::PathBuf::from(row["path"].as_str().ok_or("Missing attachment path")?)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !path.starts_with(&root) {
            return Err("Invalid attachment cache path".into());
        }
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > FILE_LIMIT as u64 {
            return Err("Choose a file under 6 MB".into());
        }
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        total += bytes.len();
        if bytes.len() > FILE_LIMIT || total > TOTAL_LIMIT {
            return Err("Attachment limit: 6 MB per file, 12 MB total".into());
        }
        let name = row["name"]
            .as_str()
            .unwrap_or("Attachment")
            .chars()
            .take(240)
            .collect::<String>();
        let mime = row["mime"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| crate::mime_for(std::path::Path::new(&name)).into());
        files.push((name, mime, bytes));
    }
    Ok(files)
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_filesPicked<'a>(
    mut env: jni::EnvUnowned<'a>,
    _this: JObject<'a>,
    token: JString<'a>,
    result: JString<'a>,
) {
    // The JNI wrapper catches panics before they can cross into Java.
    env.with_env(|env| -> jni::errors::Result<()> {
        let id = token.try_to_string(env)?;
        let json = result.try_to_string(env)?;
        finish(&id, read_result(&json));
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
