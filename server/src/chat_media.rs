//! Media delivered over the authenticated chat socket. Peer input never names a host path.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const LIMIT: usize = 6_000_000;
fn filename(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 240
        && s != "."
        && s != ".."
        && !s.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
}
fn valid_id(s: &str) -> bool {
    Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s)
}

fn mime(name: &str) -> Option<&'static str> {
    Some(
        match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "mp4" | "m4v" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "mkv" => "video/x-matroska",
            _ => return None,
        },
    )
}
fn open_at(parent: &fs::File, name: &str, directory: bool) -> io::Result<fs::File> {
    if !filename(name) {
        return Err(io::Error::other("Invalid media name"));
    }
    let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK | if directory { libc::O_DIRECTORY } else { 0 };
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(format!("/proc/self/fd/{}/{}", parent.as_raw_fd(), name))
}
fn bounded(mut file: fs::File) -> Result<Vec<u8>, String> {
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > LIMIT as u64 {
        return Err("Media must be a regular file of at most 6 MB".into());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut file)
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > LIMIT {
        return Err("Invalid media size".into());
    }
    Ok(bytes)
}
fn metadata(id: String, name: &str, mime: &str) -> Value {
    json!({"type":if mime.starts_with("image/"){"image"}else{"file"},"artifactId":id,"fileName":name,"mimeType":mime})
}
fn blob(root: &Path, bytes: &[u8], name: &str) -> Result<Value, String> {
    let mime = mime(name).ok_or("Unsupported media format")?;
    if bytes.is_empty() || bytes.len() > LIMIT {
        return Err("Invalid media size".into());
    }
    let dir = root.join("chat-media-cache");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let path = dir.join(&hash);
    if !path.is_file() {
        let tmp = dir.join(format!("temp-{}", Uuid::new_v4()));
        let result = (|| -> io::Result<()> {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?
                .write_all(bytes)?;
            fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(tmp);
            return Err(e.to_string());
        }
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|f| f.file_name().len() == 64)
            .filter_map(|f| {
                let meta = f.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), f.path()))
            })
            .collect();
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        if total > 256 * 1024 * 1024 {
            entries.sort_by_key(|e| e.0);
            for (_, len, candidate) in entries {
                if total <= 256 * 1024 * 1024 {
                    break;
                }
                if candidate != path && fs::remove_file(candidate).is_ok() {
                    total = total.saturating_sub(len);
                }
            }
        }
    }
    Ok(metadata(format!("blob/{hash}/{name}"), name, mime))
}
pub fn download(root: &Path, id: &str) -> Result<Value, String> {
    let mut parts = id.splitn(3, '/');
    let kind = parts.next().unwrap_or("");
    let key = parts.next().ok_or("Missing media identifier")?;
    let name = parts
        .next()
        .filter(|n| filename(n))
        .ok_or("Invalid attachment name")?;
    let media_type = mime(name).ok_or("Unsupported media format")?;
    let file = (|| -> io::Result<_> {
        match kind {
            "blob" if key.len() == 64 && key.bytes().all(|c| c.is_ascii_hexdigit()) => {
                open_at(&fs::File::open(root.join("chat-media-cache"))?, key, false)
            }
            "upload" if valid_id(key) => open_at(
                &open_at(&fs::File::open(root.join("uploads"))?, key, true)?,
                name,
                false,
            ),
            _ => Err(io::Error::other("Unknown media identifier")),
        }
    })()
    .map_err(|e| e.to_string())?;
    Ok(json!({"data":STANDARD.encode(bounded(file)?),"artifact":{"mimeType":media_type}}))
}
pub fn share(root: &Path, thread: &str, name: Option<&str>) -> Result<Value, String> {
    if !valid_id(thread) {
        return Err("Invalid conversation".into());
    }
    let base = root.join("codex/workspace");
    let mut dir = fs::File::open(&base).map_err(|e| e.to_string())?;
    for part in ["d4c-media", thread] {
        let path = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(part);
        if let Err(e) = fs::create_dir(&path)
            && e.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(e.to_string());
        }
        dir = open_at(&dir, part, true).map_err(|e| e.to_string())?;
    }
    let Some(name) = name else {
        return Ok(json!({"directory":base.join("d4c-media").join(thread)}));
    };
    let media_type = mime(name).ok_or("Unsupported media format")?;
    let bytes = bounded(open_at(&dir, name, false).map_err(|e| e.to_string())?)?;
    // Keep shared files recoverable from the source transcript after cache eviction.
    let p = super::transport::store_attachment(
        &root.join("uploads"),
        &json!({"name":name,"data":STANDARD.encode(bytes)}),
    )?;
    let key = Path::new(p["path"].as_str().ok_or("Missing upload path")?)
        .parent()
        .and_then(|v| v.file_name())
        .and_then(|v| v.to_str())
        .ok_or("Missing upload identifier")?;
    Ok(json!({"d4cAttachment":metadata(format!("upload/{key}/{name}"),name,media_type)}))
}
pub fn project_item(root: &Path, item: &mut Value) -> bool {
    let mut changed = false;
    if item["type"] == "imageGeneration"
        && let Some(path) = item["savedPath"].as_str()
    {
        let base = root.join("codex/generated_images");
        if let Ok(suffix) = Path::new(path).strip_prefix(&base) {
            let mut parts = suffix.iter().filter_map(|s| s.to_str());
            if let (Some(id), Some(name), None) = (parts.next(), parts.next(), parts.next())
                && valid_id(id)
            {
                let result = (|| -> Result<_, String> {
                    let base = fs::File::open(base).map_err(|e| e.to_string())?;
                    let dir = open_at(&base, id, true).map_err(|e| e.to_string())?;
                    blob(
                        root,
                        &bounded(open_at(&dir, name, false).map_err(|e| e.to_string())?)?,
                        name,
                    )
                })();
                if let Ok(media) = result {
                    item["d4cAttachment"] = media;
                    changed = true;
                }
            }
        }
    }
    if item["type"] != "dynamicToolCall" {
        return changed;
    }
    for part in item
        .get_mut("contentItems")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if part["type"] == "inputText"
            && let Some(text) = part["text"].as_str().filter(|s| s.len() < 4096)
            && let Ok(record) = serde_json::from_str::<Value>(text)
            && let Some(key) = record["d4cScreenshot"]["artifactId"]
                .as_str()
                .and_then(|id| id.strip_prefix("upload/")?.strip_suffix("/Screenshot.png"))
                .filter(|key| valid_id(key))
        {
            *part = json!({"type":"inputText","text":"Screenshot captured","d4cAttachment":metadata(format!("upload/{key}/Screenshot.png"),"Screenshot.png","image/png")});
            changed = true;
            continue;
        }
        if part["type"] == "inputImage"
            && let Some(url) = part["imageUrl"]
                .as_str()
                .filter(|u| u.starts_with("data:image/"))
        {
            let attachment = url.split_once(";base64,").and_then(|(m, b)| {
                if b.len() > 8_000_000 {
                    return None;
                }
                let ext = match m {
                    "data:image/png" => "png",
                    "data:image/jpeg" => "jpg",
                    "data:image/gif" => "gif",
                    "data:image/webp" => "webp",
                    _ => return None,
                };
                blob(
                    root,
                    &STANDARD.decode(b).ok()?,
                    &format!("Screenshot.{ext}"),
                )
                .ok()
            });
            *part = json!({"type":"inputText","text":"[Tool image saved in Codex history]"});
            if let Some(media) = attachment {
                part["d4cAttachment"] = media;
            }
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_refs_round_trip_without_allowing_path_traversal_or_symlinks() {
        let root = std::env::temp_dir().join(format!("d4c-media-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("codex/workspace")).unwrap();
        fs::create_dir(root.join("uploads")).unwrap();
        let thread = Uuid::new_v4().to_string();
        let directory = share(&root, &thread, None).unwrap();
        let dir = Path::new(directory["directory"].as_str().unwrap());
        fs::write(dir.join("clip.mp4"), b"video-bytes").unwrap();
        let r = share(&root, &thread, Some("clip.mp4")).unwrap();
        assert_eq!(
            download(&root, r["d4cAttachment"]["artifactId"].as_str().unwrap()).unwrap()["data"],
            STANDARD.encode(b"video-bytes")
        );
        std::os::unix::fs::symlink("/etc/passwd", dir.join("fake.mp4")).unwrap();
        assert!(share(&root, &thread, Some("fake.mp4")).is_err());
        assert!(download(&root, "upload/../../etc/passwd").is_err());
        let mut item = json!({"type":"dynamicToolCall","contentItems":[{"type":"inputImage","imageUrl":"data:image/png;base64,YQ=="}]});
        assert!(project_item(&root, &mut item));
        assert_eq!(
            download(
                &root,
                item["contentItems"][0]["d4cAttachment"]["artifactId"]
                    .as_str()
                    .unwrap()
            )
            .unwrap()["data"],
            "YQ=="
        );
        fs::remove_dir_all(root).unwrap();
    }
}
