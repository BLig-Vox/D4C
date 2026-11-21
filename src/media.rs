//! Bounded previews from uploads and the gateway's authenticated media route.
use base64::Engine as _;
use eframe::egui;
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[cfg(target_os = "android")]
#[path = "media/android_player.rs"]
mod backend;
#[cfg(not(target_os = "android"))]
#[path = "media/desktop_player.rs"]
mod backend;
mod viewer;
use std::io::{Read, Write};
use tempfile::NamedTempFile;
pub(crate) use viewer::open_image;
pub use viewer::{close_fullscreen, finish_frame, is_fullscreen, reset};

static WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::Relaxed);
    }
}
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub size: usize,
    pub artifact: Option<String>,
    digest: Option<[u8; 32]>,
    remote_identity: Option<String>,
    visible: AtomicBool,
    play_requested: AtomicBool,
    source: Option<(String, String)>,
    state: Mutex<State>,
}
enum Payload {
    Bytes(Arc<[u8]>),
    Http {
        endpoint: String,
        token: String,
        source: String,
        session: String,
        message: String,
    },
}
enum State {
    Pending(Payload),
    Loading(Arc<Mutex<Option<Result<Decoded, String>>>>),
    Image(egui::TextureHandle, Option<Arc<Vec<u8>>>),
    Downloading {
        file: NamedTempFile,
        received: u64,
        total: u64,
    },
    Video(Arc<NamedTempFile>),
    Error(String),
    DownloadFailed(String),
}
enum Decoded {
    Image(egui::ColorImage, Option<Arc<Vec<u8>>>),
    Video(NamedTempFile),
}

const VIDEO_LIMIT: u64 = 128 * 1024 * 1024;

fn video_temp() -> Result<NamedTempFile, String> {
    #[cfg(target_os = "android")]
    let directory = crate::platform::attachment_cache().map_err(|e| e.to_string())?;
    #[cfg(target_os = "android")]
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "android"))]
    let directory = std::env::temp_dir();
    NamedTempFile::new_in(directory).map_err(|e| e.to_string())
}

fn decode(payload: Payload, video: bool) -> Result<Decoded, String> {
    let mut input: Box<dyn Read> = match payload {
        Payload::Bytes(bytes) => Box::new(std::io::Cursor::new(bytes)),
        Payload::Http {
            endpoint,
            token,
            source,
            session,
            message,
        } => {
            let config = ureq::Agent::config_builder()
                .max_redirects(0)
                .timeout_global(Some(std::time::Duration::from_secs(90)))
                .build();
            let agent: ureq::Agent = config.into();
            let mut request = agent
                .get(&endpoint)
                .header("Authorization", format!("Bearer {token}"));
            if !source.is_empty() {
                request = request
                    .query("source", &source)
                    .query("sessionKey", session)
                    .query("messageId", message);
            }
            let response = request.call().map_err(|_| "Attachment download failed")?;
            if !response.status().is_success() {
                return Err("Attachment download failed".into());
            }
            Box::new(response.into_body().into_reader())
        }
    };
    if video {
        let mut file = video_temp()?;
        let count = std::io::copy(&mut input.take(VIDEO_LIMIT + 1), &mut file)
            .map_err(|e| e.to_string())?;
        if count == 0 || count > VIDEO_LIMIT {
            return Err("Video is empty or exceeds 128 MB".into());
        }
        file.flush().map_err(|e| e.to_string())?;
        return Ok(Decoded::Video(file));
    }
    const IMAGE_LIMIT: u64 = 32 * 1024 * 1024;
    let mut bytes = Vec::new();
    input
        .take(IMAGE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > IMAGE_LIMIT {
        return Err("Image exceeds 32 MB".into());
    }
    let (image, reduced) = decode_image(&bytes, 960)?;
    Ok(Decoded::Image(image, reduced.then(|| Arc::new(bytes))))
}

pub(crate) fn decode_image(
    bytes: &[u8],
    max_side: u32,
) -> Result<(egui::ColorImage, bool), String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Unknown image format")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| "Unsupported image or image too large")?;
    let reduced = decoded.width().max(decoded.height()) > max_side;
    let image = if reduced {
        decoded.thumbnail(max_side, max_side)
    } else {
        decoded
    }
    .to_rgba8();
    Ok((
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ),
        reduced,
    ))
}
impl Attachment {
    pub fn pending_offset(&self) -> Option<u64> {
        match &*self.state.lock().ok()? {
            State::Pending(Payload::Bytes(bytes)) if bytes.is_empty() => Some(0),
            State::Downloading { received, .. } => Some(*received),
            _ => None,
        }
    }

    pub fn download_offset(&self) -> Option<u64> {
        let offset = self.pending_offset()?;
        (offset > 0
            || (self.visible.load(Ordering::Relaxed)
                && (!self.mime.starts_with("video/")
                    || self.play_requested.load(Ordering::Relaxed))))
        .then_some(offset)
    }

    pub fn receive_video_chunk(&mut self, offset: u64, total: u64, bytes: &[u8]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let result = (|| -> Result<State, String> {
            let end = offset
                .checked_add(bytes.len() as u64)
                .ok_or("Invalid video chunk")?;
            if !self.mime.starts_with("video/")
                || total > VIDEO_LIMIT
                || bytes.is_empty()
                || end > total
            {
                return Err("Invalid video chunk".into());
            }
            let mut file = match std::mem::replace(&mut *state, State::Error(String::new())) {
                State::Pending(Payload::Bytes(data)) if data.is_empty() && offset == 0 => {
                    video_temp()?
                }
                State::Downloading {
                    file,
                    received,
                    total: expected,
                } if received == offset && expected == total => file,
                _ => return Err("Unexpected video chunk".into()),
            };
            file.write_all(bytes).map_err(|e| e.to_string())?;
            if end == total {
                file.flush().map_err(|e| e.to_string())?;
                Ok(State::Video(Arc::new(file)))
            } else {
                Ok(State::Downloading {
                    file,
                    received: end,
                    total,
                })
            }
        })();
        *state = result.unwrap_or_else(State::DownloadFailed);
    }

    pub fn cache_bytes(&self) -> usize {
        let payload = self
            .state
            .lock()
            .map(|state| match &*state {
                State::Pending(Payload::Bytes(b)) => b.len(),
                State::Pending(Payload::Http {
                    endpoint,
                    token,
                    source,
                    session,
                    message,
                }) => endpoint.len() + token.len() + source.len() + session.len() + message.len(),
                State::Loading(_) => 32 * 1024 * 1024,
                State::Image(texture, original) => texture
                    .size()
                    .iter()
                    .product::<usize>()
                    .saturating_mul(8)
                    .saturating_add(original.as_ref().map_or(0, |b| b.len())),
                State::Video(_) | State::Downloading { .. } => 256,
                State::Error(e) | State::DownloadFailed(e) => e.len(),
            })
            .unwrap_or(usize::MAX / 2);
        payload
            .saturating_add(self.name.len() + self.mime.len())
            .saturating_add(
                self.source
                    .as_ref()
                    .map(|(a, b)| a.len() + b.len())
                    .unwrap_or(0),
            )
    }

    pub fn new(name: String, mime: String, bytes: Vec<u8>) -> Self {
        let mime = if mime.is_empty() || mime == "application/octet-stream" {
            media_mime(&name)
                .unwrap_or("application/octet-stream")
                .into()
        } else {
            mime
        };
        use sha2::Digest;
        let digest = (!bytes.is_empty()).then(|| sha2::Sha256::digest(&bytes).into());
        Self {
            name,
            mime,
            size: bytes.len(),
            artifact: None,
            digest,
            remote_identity: None,
            visible: AtomicBool::new(false),
            play_requested: AtomicBool::new(false),
            source: None,
            state: Mutex::new(State::Pending(Payload::Bytes(bytes.into()))),
        }
    }
    pub fn download_failed(&mut self, message: &str) {
        if let Ok(mut state) = self.state.lock() {
            *state = State::DownloadFailed(message.chars().take(300).collect());
        }
    }
    pub fn receive(&mut self, mime: String, bytes: Arc<[u8]>) {
        if self.mime == "application/octet-stream"
            || mime.starts_with("image/")
            || mime.starts_with("video/")
        {
            self.mime = mime;
        }
        self.size = bytes.len();
        if let Ok(mut state) = self.state.lock() {
            *state = State::Pending(Payload::Bytes(bytes));
        }
    }
    pub fn receive_url(&mut self, gateway: &str, token: &str, path: &str) {
        self.download_failed("Invalid attachment download URL");
        let Ok(uri) = gateway.parse::<tungstenite::http::Uri>() else {
            return;
        };
        let Some(authority) = uri.authority() else {
            return;
        };
        if uri.scheme_str() != Some("wss")
            || !(path.starts_with("/api/chat/media/outgoing/")
                || path.starts_with("/__openclaw__/"))
            || path.len() > 8192
        {
            return;
        }
        self.source = None;
        if let Ok(mut state) = self.state.lock() {
            *state = State::Pending(Payload::Http {
                endpoint: format!("https://{authority}{path}"),
                token: token.into(),
                source: String::new(),
                session: String::new(),
                message: String::new(),
            });
        }
    }
    pub fn configure_http(&mut self, gateway: &str, token: &str, session: &str) {
        if self.source.is_none()
            || !(self.mime.starts_with("image/") || self.mime.starts_with("video/"))
        {
            return;
        }
        let Ok(uri) = gateway.parse::<tungstenite::http::Uri>() else {
            return;
        };
        if uri.scheme_str() != Some("wss") {
            return;
        }
        let Some(authority) = uri.authority() else {
            return;
        };
        let (source, message) = self.source.take().unwrap();
        self.remote_identity = Some(source.clone());
        if let Ok(mut state) = self.state.lock() {
            *state = State::Pending(Payload::Http {
                endpoint: format!("https://{authority}/__openclaw__/assistant-media"),
                token: token.into(),
                source,
                session: session.into(),
                message,
            });
        }
    }
    pub fn ui(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::icon_text(
                if self.mime.starts_with("image/") {
                    lucide_icons::Icon::Image
                } else {
                    lucide_icons::Icon::File
                },
                16.0,
            ));
            ui.label(
                if self.name.starts_with("d4c-") && self.name.ends_with(".png") {
                    "Desktop screenshot"
                } else {
                    &self.name
                },
            );
            if self.size > 0 {
                ui.label(
                    egui::RichText::new(format!("{:.1} KB", self.size as f32 / 1000.0))
                        .small()
                        .weak(),
                );
            }
        });
        let video = self.mime.starts_with("video/");
        if !(video || self.mime.starts_with("image/")) {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let width = ui.available_width().min(420.).max(1.);
        let image_space = egui::vec2(width, (width * 0.5625).max(48.));
        let visible = ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, image_space));
        self.visible.store(visible, Ordering::Relaxed);
        if matches!(*state, State::Pending(_))
            && video
            && !self.play_requested.load(Ordering::Relaxed)
        {
            if ui.button("Play video").clicked() {
                self.play_requested.store(true, Ordering::Relaxed);
                ui.ctx().request_repaint();
            } else {
                return;
            }
        }
        if matches!(&*state, State::Pending(Payload::Bytes(b)) if b.is_empty()) {
            if self.source.is_some() || self.artifact.is_some() {
                if visible {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(200));
                }
                ui.weak(if video {
                    "Starting download…"
                } else {
                    "Waiting for attachment…"
                });
            } else {
                ui.weak("Attachment unavailable");
            }
            return;
        }
        if matches!(*state, State::Pending(_)) && visible && self.source.is_none() {
            if WORKERS
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    (n < 4).then_some(n + 1)
                })
                .is_ok()
            {
                let State::Pending(payload) =
                    std::mem::replace(&mut *state, State::Error(String::new()))
                else {
                    unreachable!()
                };
                let result = Arc::new(Mutex::new(None));
                let output = result.clone();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let _permit = Permit;
                    let decoded = decode(payload, video);
                    if let Ok(mut slot) = output.lock() {
                        *slot = Some(decoded);
                    }
                    ctx.request_repaint();
                });
                *state = State::Loading(result);
            }
        }
        if let State::Loading(result) = &*state {
            let decoded = result.lock().ok().and_then(|mut r| r.take());
            if let Some(result) = decoded {
                *state = match result {
                    Ok(Decoded::Image(image, original)) => State::Image(
                        ui.ctx()
                            .load_texture(&self.name, image, egui::TextureOptions::LINEAR),
                        original,
                    ),
                    Ok(Decoded::Video(file)) => State::Video(Arc::new(file)),
                    Err(error) => State::Error(error),
                }
            }
        }
        match &*state {
            State::Image(texture, original) => {
                let size = texture.size_vec2();
                let scale = (width / size.x).min(320. / size.y).min(1.);
                if ui
                    .add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(size * scale)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Open full screen")
                    .clicked()
                {
                    viewer::open_image(self.name.clone(), texture.clone(), original.clone());
                }
            }
            State::Downloading {
                received, total, ..
            } => {
                ui.add(
                    egui::ProgressBar::new(*received as f32 / *total as f32)
                        .desired_width(width)
                        .text(format!(
                            "Downloading video · {}%",
                            *received * 100 / *total
                        )),
                );
            }
            State::Video(file) => {
                if visible && self.play_requested.swap(false, Ordering::Relaxed) {
                    viewer::play(file.clone(), self.name.clone());
                }
                viewer::inline(ui, &self.name, file);
            }
            State::DownloadFailed(error) => {
                ui.weak(error);
                if ui.button("Retry download").clicked() {
                    *state = State::Pending(Payload::Bytes(Arc::from([])));
                    ui.ctx().request_repaint();
                }
            }
            State::Error(error) => {
                ui.weak(error);
            }
            _ => {
                ui.allocate_ui(image_space, |ui| {
                    ui.spinner();
                });
            }
        }
    }
}
pub(crate) fn media_mime(name: &str) -> Option<&'static str> {
    Some(
        match name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "mp4" | "m4v" => "video/mp4",
            "mov" => "video/quicktime",
            "webm" => "video/webm",
            "mkv" => "video/x-matroska",
            _ => return None,
        },
    )
}
/// Keep completed and in-progress media when a tail refresh replaces its row.
pub fn retain_loaded(new: &mut [Attachment], old: &mut Vec<Attachment>) {
    for attachment in new {
        let origin = |a: &Attachment| {
            a.source
                .as_ref()
                .map(|s| s.0.clone())
                .or_else(|| a.remote_identity.clone())
        };
        let origin_new = origin(attachment);
        if let Some(index) = old.iter().position(|prior| {
            attachment.artifact.is_some() && attachment.artifact == prior.artifact
                || attachment.digest.is_some() && attachment.digest == prior.digest
                || origin_new.is_some() && origin_new == origin(prior)
        }) {
            *attachment = old.remove(index);
        }
    }
}

pub fn from_message(message: &Value) -> Vec<Attachment> {
    let mut out: Vec<_> = message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| {
            let kind = part["type"].as_str()?;
            if !matches!(
                kind,
                "image" | "image_url" | "video" | "video_url" | "file" | "document"
            ) {
                return None;
            }
            let mime = part["mimeType"]
                .as_str()
                .or_else(|| part["source"]["media_type"].as_str())
                .or_else(|| part["mime_type"].as_str())
                .unwrap_or(if kind.starts_with("image") {
                    "image/png"
                } else if kind.starts_with("video") {
                    "video/mp4"
                } else {
                    "application/octet-stream"
                })
                .to_owned();
            let name = part["fileName"]
                .as_str()
                .or_else(|| part["name"].as_str())
                .or_else(|| part["alt"].as_str())
                .unwrap_or(if mime.starts_with("image/") {
                    "Image"
                } else {
                    "File"
                })
                .to_owned();
            let encoded = part["data"]
                .as_str()
                .or_else(|| part["source"]["data"].as_str())
                .or_else(|| part["image_url"]["url"].as_str())
                .or_else(|| part["video_url"]["url"].as_str())
                .unwrap_or("");
            let encoded = if encoded.starts_with("data:") {
                encoded.split_once(',').map(|(_, d)| d).unwrap_or("")
            } else {
                encoded
            };
            let bytes = if encoded.len() <= 8_000_000 {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap_or_default()
            } else {
                vec![]
            };
            if bytes.is_empty()
                && part["artifactId"].as_str().is_none()
                && part["url"].as_str().is_none()
                && (part["omitted"] == true || part["source"]["omitted"] == true)
            {
                return None;
            }
            let mut a = Attachment::new(name, mime, bytes);
            a.artifact = part["artifactId"].as_str().map(str::to_owned);
            if a.artifact.is_none() {
                if let Some(source) = part["url"]
                    .as_str()
                    .or_else(|| part["source"].as_str())
                    .or_else(|| part["source"]["url"].as_str())
                    .or_else(|| part["image_url"]["url"].as_str())
                    .or_else(|| part["video_url"]["url"].as_str())
                    .filter(|s| {
                        s.len() < 2048
                            && (s.starts_with('/')
                                || s.starts_with("file:///")
                                || s.starts_with("media://inbound/"))
                    })
                {
                    a.source = Some((
                        source.into(),
                        message["__openclaw"]["id"].as_str().unwrap_or("").into(),
                    ));
                    if a.name == "Image" {
                        a.name = source.rsplit('/').next().unwrap_or("Image").into();
                    }
                }
            }
            Some(a)
        })
        .take(8)
        .collect();
    for fact in message["__openclaw"]["media"]
        .as_array()
        .into_iter()
        .flatten()
        .take(8 - out.len())
    {
        let mime = fact["contentType"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_owned();
        let name = fact["fileName"].as_str().unwrap_or("Attachment").to_owned();
        let mut a = Attachment::new(name, mime, vec![]);
        a.size = fact["sizeBytes"].as_u64().unwrap_or(0) as usize;
        if let Some(uri) = fact["url"].as_str().filter(|u| {
            (u.starts_with("media://inbound/") || u.starts_with('/') || u.starts_with("file:///"))
                && u.len() < 2048
        }) {
            a.source = Some((
                uri.into(),
                message["__openclaw"]["id"].as_str().unwrap_or("").into(),
            ));
        }
        out.push(a);
    }
    // MEDIA references are delivered through the authenticated Gateway route,
    // never opened as client-local files or fetched from arbitrary remote hosts.
    if message["role"] == "assistant" {
        for text in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["text"].as_str())
        {
            for line in text.lines().filter_map(|l| l.trim().strip_prefix("MEDIA:")) {
                let source = line.trim().trim_matches(['`', '"', '\'']);
                if out.len() >= 8
                    || source.len() > 2048
                    || !(source.starts_with('/') || source.starts_with("file:///"))
                {
                    continue;
                }
                if out
                    .iter()
                    .any(|a| a.source.as_ref().is_some_and(|(s, _)| s == source))
                {
                    continue;
                }
                let Some(mime) = media_mime(source) else {
                    continue;
                };
                let mut a = Attachment::new(
                    source.rsplit('/').next().unwrap_or("Screenshot").into(),
                    mime.into(),
                    vec![],
                );
                a.source = Some((
                    source.into(),
                    message["__openclaw"]["id"].as_str().unwrap_or("").into(),
                ));
                out.push(a);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn omitted_tool_observation_does_not_render_a_broken_preview() {
        assert!(from_message(&json!({"role":"toolResult","content":[{"type":"image","mimeType":"image/png","omitted":true}]})).is_empty());
    }
    #[test]
    fn managed_attachment_retains_download_identity() {
        let a = from_message(
            &json!({"role":"assistant","content":[{"type":"image","artifactId":"artifact_managed_image_example","mimeType":"image/png","alt":"Screenshot.png"}]}),
        );
        assert_eq!(a.len(), 1);
        assert_eq!(
            a[0].artifact.as_deref(),
            Some("artifact_managed_image_example")
        );
        assert_eq!(a[0].name, "Screenshot.png");
    }
    #[test]
    fn local_media_reference_is_gateway_scoped_and_remote_url_is_not_proxied() {
        let a = from_message(
            &json!({"role":"assistant","content":[{"type":"text","text":"MEDIA:/example-state/media/test.png\nMEDIA:https://untrusted.example/test.png"}]}),
        );
        assert_eq!(a.len(), 1);
        assert_eq!(
            a[0].source.as_ref().unwrap().0,
            "/example-state/media/test.png"
        );
    }
}
