//! Server-owned workspace connection. Closing this client never sends desktop stop.
use crate::{gateway, rfb};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::Signer;
use eframe::egui;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io,
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};
pub(crate) type Socket = WebSocket<MaybeTlsStream<TcpStream>>;
pub enum Event {
    Data(Value),
    Status(String),
    Frame(u16, u16, Vec<u8>),
}
pub struct Link {
    tx: SyncSender<Value>,
    pub rx: Receiver<Event>,
    pub frame: Arc<std::sync::Mutex<Option<(u16, u16, Vec<u8>)>>>,
    stop: Arc<AtomicBool>,
    online: Arc<AtomicBool>,
    reconnect: Arc<AtomicBool>,
}
impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Link {
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed) && !self.reconnect.load(Ordering::Relaxed)
    }
    pub fn reconnect(&self) {
        self.online.store(false, Ordering::Relaxed);
        self.reconnect.store(true, Ordering::Relaxed);
    }
    pub fn rpc(&self, method: &str, params: Value) -> bool {
        if !self.online.load(Ordering::Relaxed) {
            return false;
        }
        self.rpc_identified(method, params).is_some()
    }
    fn rpc_identified(&self, method: &str, params: Value) -> Option<String> {
        if !self.online.load(Ordering::Relaxed) {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.tx
            .try_send(json!({"id":id,"method":method,"params":params}))
            .ok()?;
        Some(id)
    }
}
pub(crate) fn tcp(socket: &mut Socket) -> Option<&mut TcpStream> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(&mut s.sock),
        _ => None,
    }
}
fn transient(e: &tungstenite::Error) -> bool {
    matches!(e,tungstenite::Error::Io(e) if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::TimedOut))
}
pub fn base(gateway: &str) -> Option<String> {
    if let Some(url) = crate::preferences::Settings::load()
        .workspace_urls
        .get(gateway)
    {
        if crate::preferences::valid_url(url) {
            return Some(url.clone());
        }
    }
    let uri: tungstenite::http::Uri = gateway.parse().ok()?;
    if uri.scheme_str() != Some("wss") {
        return None;
    }
    Some(format!("wss://{}:8443", uri.host()?))
}
pub(crate) fn open(url: &str) -> Result<Socket, String> {
    let uri: tungstenite::http::Uri = url.parse().map_err(|_| "Invalid service URL")?;
    let host = uri.host().ok_or("Service hostname missing")?;
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });
    let address_host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let addresses = (address_host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut stream = None;
    let mut last_error = "Service address unavailable".to_owned();
    for address in addresses {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&address, remaining.min(Duration::from_secs(5))) {
            Ok(socket) => {
                stream = Some(socket);
                break;
            }
            Err(error) => last_error = error.to_string(),
        }
    }
    let stream = stream.ok_or(last_error)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let (mut socket, _) = tungstenite::client_tls(url, stream).map_err(|e| e.to_string())?;
    if let Some(s) = tcp(&mut socket) {
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_nodelay(true);
    }
    let text = socket
        .read()
        .map_err(|e| e.to_string())?
        .into_text()
        .map_err(|e| e.to_string())?;
    let challenge: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if challenge["type"] != "challenge" || challenge["hostname"] != host {
        return Err("Unexpected server identity".into());
    }
    let nonce = challenge["nonce"]
        .as_str()
        .filter(|n| n.len() <= 64)
        .ok_or("Challenge missing")?;
    let key = gateway::key().map_err(|e| e.to_string())?;
    let public = key.verifying_key().to_bytes();
    let signature = key.sign(format!("d4c-service-v1|{host}|{nonce}").as_bytes());
    socket.send(Message::Text(json!({"deviceId":format!("{:x}",Sha256::digest(public)),"publicKey":URL_SAFE_NO_PAD.encode(public),"signature":URL_SAFE_NO_PAD.encode(signature.to_bytes())}).to_string().into())).map_err(|e|e.to_string())?;
    let reply: Value = serde_json::from_str(
        &socket
            .read()
            .map_err(|e| e.to_string())?
            .into_text()
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if reply["type"] == "pairing_required" {
        return Err(format!(
            "Approve this device on your workspace server: {}",
            reply["deviceId"].as_str().unwrap_or("")
        ));
    }
    if reply["type"] != "authenticated" {
        return Err("Workspace authentication rejected".into());
    }
    if let Some(s) = tcp(&mut socket) {
        let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
    }
    Ok(socket)
}
pub fn connect(base: String, desktop: Option<String>, ctx: egui::Context) -> Link {
    let (tx, commands) = mpsc::sync_channel::<Value>(64);
    let (events, rx) = mpsc::sync_channel(16);
    let stop = Arc::new(AtomicBool::new(false));
    let online = Arc::new(AtomicBool::new(false));
    let connected = online.clone();
    let reconnect = Arc::new(AtomicBool::new(false));
    let retry = reconnect.clone();
    let frame = Arc::new(std::sync::Mutex::new(None));
    let newest = frame.clone();
    let cancel = stop.clone();
    std::thread::spawn(move || {
        let emit = |event| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            if let Event::Frame(w, h, pixels) = event {
                if let Ok(mut frame) = newest.lock() {
                    *frame = Some((w, h, pixels));
                    ctx.request_repaint();
                    return true;
                }
                return false;
            }
            let result = events.try_send(event).is_ok();
            ctx.request_repaint();
            result
        };
        while !cancel.load(Ordering::Relaxed) {
            retry.store(false, Ordering::Relaxed);
            emit(Event::Status("Connecting…".into()));
            let url = format!(
                "{base}/{}",
                desktop
                    .as_ref()
                    .map(|id| format!("vnc/{id}"))
                    .unwrap_or("control".into())
            );
            let result = (|| -> Result<(), String> {
                let mut socket = open(&url)?;
                connected.store(true, Ordering::Relaxed);
                emit(Event::Status("Online".into()));
                let mut decoder = None;
                let mut last_frame = std::time::Instant::now();
                let mut pending = false;
                let mut last_received = std::time::Instant::now();
                let mut last_ping = std::time::Instant::now();
                let framebuffer_deadline = std::time::Instant::now() + Duration::from_secs(15);
                let mut received_frame = false;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        let _ = socket.close(None);
                        return Ok(());
                    }
                    if retry.load(Ordering::Relaxed) {
                        let _ = socket.close(None);
                        return Err("Reconnect requested".into());
                    }
                    // Detect half-open mobile sockets even when the desktop is idle.
                    if last_received.elapsed() >= Duration::from_secs(30) {
                        return Err("Workspace connection timed out".into());
                    }
                    if desktop.is_some()
                        && !received_frame
                        && std::time::Instant::now() >= framebuffer_deadline
                    {
                        return Err("Desktop did not send a framebuffer".into());
                    }
                    if last_ping.elapsed() >= Duration::from_secs(10) {
                        socket
                            .send(Message::Ping(Vec::new().into()))
                            .map_err(|e| e.to_string())?;
                        last_ping = std::time::Instant::now();
                    }
                    while let Ok(req) = commands.try_recv() {
                        socket
                            .send(Message::Text(req.to_string().into()))
                            .map_err(|e| e.to_string())?;
                    }
                    let incoming = socket.read();
                    if incoming.is_ok() {
                        last_received = std::time::Instant::now();
                    }
                    match incoming {
                        Ok(Message::Text(t)) => {
                            let v: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                            if v["type"] == "framebuffer" {
                                decoder = Some(
                                    rfb::Decoder::new(
                                        v["width"].as_u64().ok_or("Width missing")? as u16,
                                        v["height"].as_u64().ok_or("Height missing")? as u16,
                                    )
                                    .map_err(|e| e.to_string())?,
                                );
                            } else {
                                // Durable messages cannot be dropped: wait briefly for the UI, with cancellation.
                                let mut event = Event::Data(v);
                                loop {
                                    match events.try_send(event) {
                                        Ok(()) => {
                                            ctx.request_repaint();
                                            break;
                                        }
                                        Err(mpsc::TrySendError::Full(e)) => {
                                            event = e;
                                            if cancel.load(Ordering::Relaxed) {
                                                return Ok(());
                                            }
                                            std::thread::sleep(Duration::from_millis(10));
                                        }
                                        Err(_) => return Ok(()),
                                    }
                                }
                            }
                        }
                        Ok(Message::Binary(bytes)) => {
                            if let Some(d) = decoder.as_mut() {
                                pending |= d.feed(&bytes).map_err(|e| e.to_string())?;
                                received_frame |= pending;
                            }
                        }
                        Ok(Message::Close(_)) => return Err("Workspace disconnected".into()),
                        Ok(_) => {}
                        Err(e) if transient(&e) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                    if pending && last_frame.elapsed() >= Duration::from_millis(33) {
                        if let Some(d) = decoder.as_ref() {
                            if emit(Event::Frame(d.width, d.height, d.rgba.clone())) {
                                pending = false;
                                last_frame = std::time::Instant::now();
                            }
                        }
                    }
                }
            })();
            connected.store(false, Ordering::Relaxed);
            // Never publish an old decoder's final image as the new stream's first frame.
            if let Ok(mut frame) = newest.lock() {
                *frame = None;
            }
            while commands.try_recv().is_ok() {}
            if let Err(e) = result {
                if desktop.is_some() {
                    eprintln!("Desktop stream disconnected: {e}");
                    let status = if e.starts_with("Approve this device")
                        || e.contains("authentication")
                        || e == "Unexpected server identity"
                    {
                        e
                    } else {
                        "Desktop disconnected. Reconnecting…".into()
                    };
                    emit(Event::Status(status));
                } else {
                    emit(Event::Status(e));
                }
            }
            for _ in 0..30 {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if retry.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
    Link {
        tx,
        rx,
        stop,
        frame,
        online,
        reconnect,
    }
}
#[derive(Default)]
pub struct Workspace {
    pub link: Option<Link>,
    pub status: String,
    pub objects: std::collections::HashMap<(String, String), Value>,
    pub revision: u64,
    pub ready: bool,
    pub error: String,
    pub device: Option<String>,
    pending_control: std::collections::HashMap<String, (String, bool)>,
    pending_writes: std::collections::HashMap<String, (String, String)>,
}
impl Workspace {
    pub fn connect(&mut self, url: &str, ctx: &egui::Context) {
        self.device = gateway::key()
            .ok()
            .map(|k| format!("{:x}", Sha256::digest(k.verifying_key().to_bytes())));
        if let Some(base) = base(url) {
            self.link = Some(connect(base, None, ctx.clone()));
            self.status = "Connecting…".into();
            self.objects.clear();
            self.revision = 0;
            self.ready = false;
        }
    }
    pub fn control_pending(&self, desktop: &str) -> bool {
        self.pending_control
            .values()
            .any(|(id, handoff)| id == desktop && *handoff)
    }
    pub fn rpc(&mut self, method: &str, p: Value) {
        let control = matches!(method, "desktop.claim" | "desktop.return_agent");
        let desktop = p["id"].as_str().unwrap_or("").to_owned();
        if (control || method == "desktop.input") && self.control_pending(&desktop) {
            return;
        }
        let write = (method == "workspace.put")
            .then(|| (p["kind"].as_str().unwrap_or("").to_owned(), desktop.clone()));
        if write.is_some() && self.pending_writes.len() >= 512 {
            self.error = "Workspace save queue is busy; retrying".into();
            return;
        }
        // Renewing our existing lease must not drop a held pointer/key release.
        let renewal = method == "desktop.claim"
            && self.get("desktop", &desktop).is_some_and(|d| {
                d["mode"] == "human" && d["controller"].as_str() == self.device.as_deref()
            });
        if let Some(id) = self.link.as_ref().and_then(|l| l.rpc_identified(method, p)) {
            if let Some(object) = write {
                self.pending_writes.insert(id.clone(), object);
            }
            if control {
                self.pending_control.insert(id, (desktop, !renewal));
            }
        } else {
            self.error = "Workspace unavailable or busy".into();
        }
    }
    pub fn get(&self, kind: &str, id: &str) -> Option<&Value> {
        self.objects.get(&(kind.into(), id.into()))
    }
    pub fn list(&self, kind: &str) -> Vec<Value> {
        let mut values: Vec<_> = self
            .objects
            .iter()
            .filter(|((k, _), v)| k == kind && v["deleted"] != true)
            .map(|(_, v)| v.clone())
            .collect();
        values.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_lowercase());
        values
    }
    pub fn poll(&mut self) {
        let events: Vec<_> = self
            .link
            .as_ref()
            .map(|l| l.rx.try_iter().collect())
            .unwrap_or_default();
        for e in events {
            match e {
                Event::Status(s) => {
                    if s != "Online" {
                        self.pending_control.clear();
                        self.pending_writes.clear();
                    }
                    self.status = s;
                }
                Event::Data(v) => {
                    let write = if v["type"] == "response" {
                        v["id"]
                            .as_str()
                            .and_then(|id| self.pending_writes.remove(id))
                    } else {
                        None
                    };
                    let write_conflict = write.is_some()
                        && v["error"]
                            .as_str()
                            .is_some_and(|error| error.starts_with("conflict:"));
                    if let Some((kind, id)) = write {
                        if v["ok"] == true {
                            self.apply_object(kind, id, v["payload"].clone());
                            if self.error.starts_with("conflict:") {
                                self.error.clear();
                            }
                        }
                    }
                    if v["type"] == "response" {
                        if let Some((desktop, _)) = v["id"]
                            .as_str()
                            .and_then(|id| self.pending_control.remove(id))
                        {
                            if v["ok"] == true {
                                // Use the authoritative acknowledgement immediately;
                                // don't wait for the next changes broadcast.
                                if v["payload"]["id"].as_str() == Some(desktop.as_str()) {
                                    self.objects
                                        .insert(("desktop".into(), desktop), v["payload"].clone());
                                }
                                if matches!(
                                    self.error.as_str(),
                                    "controller_lease_required"
                                        | "controller_busy"
                                        | "human_takeover: agent input is paused"
                                ) {
                                    self.error.clear();
                                }
                            }
                        }
                    }
                    if v["type"] == "response" && v["ok"] == false && !write_conflict {
                        self.error = v["error"].as_str().unwrap_or("Operation failed").into();
                    }
                    if v["type"] == "snapshot" {
                        self.snapshot(&v["payload"]);
                    }
                    if v["type"] == "changes" {
                        let p = &v["payload"];
                        if p["resync"] == true {
                            self.snapshot(&p["snapshot"]);
                        } else {
                            for e in p["events"].as_array().into_iter().flatten() {
                                if let (Some(k), Some(id)) = (e["kind"].as_str(), e["id"].as_str())
                                {
                                    let mut value = e["value"].clone();
                                    value["rev"] = e["seq"].clone();
                                    self.apply_object(k.into(), id.into(), value);
                                }
                            }
                            self.revision = p["revision"].as_u64().unwrap_or(self.revision);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    fn apply_object(&mut self, kind: String, id: String, value: Value) {
        let key = (kind, id);
        let incoming = value["rev"].as_u64().unwrap_or(0);
        if self
            .objects
            .get(&key)
            .is_none_or(|old| old["rev"].as_u64().unwrap_or(0) <= incoming)
        {
            self.objects.insert(key, value);
        }
    }
    fn snapshot(&mut self, p: &Value) {
        self.ready = true;
        self.objects.clear();
        if let Some(o) = p.as_object() {
            for (k, items) in o {
                for value in items.as_array().into_iter().flatten() {
                    if let Some(id) = value["id"].as_str() {
                        self.objects.insert((k.clone(), id.into()), value.clone());
                    }
                }
            }
        }
        self.revision = p["revision"].as_u64().unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_restores_server_workspace() {
        let mut w = Workspace::default();
        w.snapshot(&json!({"revision":9,"desktop":[{"id":"d","name":"Work"}],"draft":[{"id":"session","text":"Unsent"}],"binding":[{"id":"session","desktopId":"d"}]}));
        assert!(w.ready);
        assert_eq!(w.revision, 9);
        assert_eq!(w.get("draft", "session").unwrap()["text"], "Unsent");
        assert_eq!(w.list("desktop").len(), 1);
        w.snapshot(&json!({"revision":12,"desktop":[]}));
        assert!(w.list("desktop").is_empty());
    }
    #[test]
    fn service_url_preserves_server_hostname() {
        assert_eq!(
            base("wss://gateway.example/path").as_deref(),
            Some("wss://gateway.example:8443")
        );
        assert!(base("ws://insecure.example").is_none());
    }
}
