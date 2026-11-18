use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};
use tungstenite::{Message, stream::MaybeTlsStream};

#[derive(Debug)]
pub enum Event {
    Status(String),
    Data(Value),
    Error(String),
}
pub struct Gateway {
    pub tx: Sender<Value>,
    pub rx: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
pub fn test_gateway(tx: Sender<Value>, rx: Receiver<Event>) -> Gateway {
    Gateway {
        tx,
        rx,
        cancelled: Arc::new(AtomicBool::new(false)),
    }
}

pub(crate) fn key() -> io::Result<SigningKey> {
    let dir = crate::platform::config_directory()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("device.key");
    match fs::read(&path) {
        Ok(bytes) => {
            let key: [u8; 32] = bytes
                .try_into()
                .map_err(|_| io::Error::other("Invalid device key"))?;
            Ok(SigningKey::from_bytes(&key))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = SigningKey::generate(&mut OsRng);
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(path)?;
            file.write_all(&key.to_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e),
    }
}
fn timeout(stream: &mut MaybeTlsStream<TcpStream>) {
    let tcp = match stream {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(&mut s.sock),
        _ => None,
    };
    if let Some(s) = tcp {
        let _ = s.set_read_timeout(Some(Duration::from_millis(150)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
    }
}
/// Bound TCP and TLS/WebSocket startup; a black-holed route must not leave
/// the app waiting forever before its automatic reconnect loop can run.
fn open(
    url: &str,
) -> Result<tungstenite::WebSocket<MaybeTlsStream<TcpStream>>, Box<dyn std::error::Error>> {
    let uri: tungstenite::http::Uri = url.parse()?;
    let host = uri.host().ok_or("Gateway hostname missing")?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut last_error =
        io::Error::new(io::ErrorKind::TimedOut, "Gateway TCP connection timed out");
    for address in (host, uri.port_u16().unwrap_or(443)).to_socket_addrs()? {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&address, remaining.min(Duration::from_secs(5))) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                stream.set_write_timeout(Some(Duration::from_secs(30)))?;
                return tungstenite::client_tls(url, stream)
                    .map(|(socket, _)| socket)
                    .map_err(|e| e.to_string().into());
            }
            Err(error) => last_error = error,
        }
    }
    Err(last_error.into())
}
fn request(method: &str, params: Value) -> Value {
    json!({"type":"req","id":format!("{method}:{}",uuid::Uuid::new_v4()),"method":method,"params":params})
}
pub fn connect(url: String, token: String, ctx: eframe::egui::Context) -> Gateway {
    connect_mode(url, token, ctx, false)
}
/// A metadata-only catalog connection: no transcript subscription or tool streams.
pub fn connect_catalog(url: String, ctx: eframe::egui::Context) -> Gateway {
    connect_mode(url, String::new(), ctx, true)
}
fn connect_mode(url: String, token: String, ctx: eframe::egui::Context, catalog: bool) -> Gateway {
    let (out_tx, out_rx) = mpsc::channel::<Value>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    thread::spawn(move || {
        let emit = |event| {
            let _ = event_tx.send(event);
            ctx.request_repaint();
        };
        let run = || -> Result<(), Box<dyn std::error::Error>> {
            if !url.starts_with("wss://") {
                return Err("A secure wss:// Gateway URL is required".into());
            }
            emit(Event::Status("Connecting…".into()));
            let mut socket = open(&url)?;
            timeout(socket.get_mut());
            emit(Event::Status("Waiting for server…".into()));
            let handshake = std::time::Instant::now();
            let challenge: Value = loop {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if handshake.elapsed() > Duration::from_secs(45) {
                    return Err("Gateway challenge timed out".into());
                }
                match socket.read() {
                    Ok(Message::Text(t)) => {
                        let v: Value = serde_json::from_str(&t)?;
                        if v["event"] == "connect.challenge" {
                            break v;
                        }
                    }
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            };
            let nonce = challenge["payload"]["nonce"]
                .as_str()
                .ok_or("Missing Gateway challenge")?;
            let signed_at = challenge["payload"]["ts"]
                .as_i64()
                .ok_or("Missing Gateway challenge time")?;
            let signing = key()?;
            let public = signing.verifying_key().to_bytes();
            let device_id = format!("{:x}", Sha256::digest(public));
            let scopes: &[&str] = if catalog {
                &["operator.read"]
            } else {
                &[
                    "operator.read",
                    "operator.write",
                    "operator.admin",
                    "operator.questions",
                ]
            };
            let payload = format!(
                "v2|{}|gateway-client|ui|operator|{}|{}|{}|{}",
                device_id,
                scopes.join(","),
                signed_at,
                token,
                nonce
            );
            let sig = signing.sign(payload.as_bytes());
            let mut params = json!({"minProtocol":4,"maxProtocol":4,"client":{"id":"gateway-client","displayName":"D4C","version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"mode":"ui"},"role":"operator","scopes":scopes,"caps":if catalog{json!([])}else{json!(["tool-events"])},"device":{"id":device_id,"publicKey":URL_SAFE_NO_PAD.encode(public),"signature":URL_SAFE_NO_PAD.encode(sig.to_bytes()),"signedAt":signed_at,"nonce":nonce}});
            if !token.is_empty() {
                params["auth"] = json!({"token":token});
            }
            crate::wire::send(
                &mut socket,
                Message::Text(
                    json!({"type":"req","id":"connect","method":"connect","params":params})
                        .to_string()
                        .into(),
                ),
            )?;
            emit(Event::Status("Authenticating…".into()));
            let authentication_started = std::time::Instant::now();
            let mut authenticated = false;
            let mut last_received = std::time::Instant::now();
            let mut last_ping = std::time::Instant::now();
            let mut catalog_pending = false;
            let mut catalog_dirty = false;
            let mut catalog_refreshed = std::time::Instant::now();
            loop {
                if stop.load(Ordering::Relaxed) {
                    let _ = socket.close(None);
                    return Ok(());
                }
                if !authenticated && authentication_started.elapsed() >= Duration::from_secs(45) {
                    return Err("Gateway authentication timed out; reconnecting".into());
                }
                if authenticated && last_received.elapsed() >= Duration::from_secs(90) {
                    return Err("Gateway stopped responding; reconnecting".into());
                }
                if authenticated && last_ping.elapsed() >= Duration::from_secs(15) {
                    crate::wire::send(&mut socket, Message::Ping(Vec::new().into()))?;
                    last_ping = std::time::Instant::now();
                }
                if authenticated
                    && catalog
                    && !catalog_pending
                    && (catalog_dirty && catalog_refreshed.elapsed() >= Duration::from_secs(1)
                        || catalog_refreshed.elapsed() >= Duration::from_secs(30))
                {
                    crate::wire::send(
                        &mut socket,
                        Message::Text(
                            request("sessions.list", json!({"includeDerivedTitles":true}))
                                .to_string()
                                .into(),
                        ),
                    )?;
                    catalog_pending = true;
                    catalog_dirty = false;
                    catalog_refreshed = std::time::Instant::now();
                }
                // RPCs queued across a reconnect must wait for the connect reply.
                if authenticated {
                    loop {
                        match out_rx.try_recv() {
                            Ok(req) => crate::wire::send(
                                &mut socket,
                                Message::Text(req.to_string().into()),
                            )?,
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                        }
                    }
                }
                let received = socket.read();
                if received.is_ok() {
                    last_received = std::time::Instant::now();
                }
                match received {
                    Ok(Message::Text(t)) => {
                        let v: Value = serde_json::from_str(&t)?;
                        if v["id"] == "connect" {
                            if v["ok"] == true {
                                authenticated = true;
                                emit(Event::Status("Connected".into()));
                                let methods: &[&str] = if catalog {
                                    &["sessions.list", "sessions.subscribe"]
                                } else {
                                    &[
                                        "sessions.list",
                                        "sessions.subscribe",
                                        "models.list",
                                        "question.list",
                                    ]
                                };
                                for &method in methods {
                                    crate::wire::send(
                                        &mut socket,
                                        Message::Text(
                                            request(
                                                method,
                                                if method == "sessions.list" {
                                                    json!({"includeDerivedTitles":true})
                                                } else if method == "sessions.subscribe" {
                                                    json!({"includeMessageBodies":!catalog})
                                                } else {
                                                    json!({})
                                                },
                                            )
                                            .to_string()
                                            .into(),
                                        ),
                                    )?;
                                }
                                catalog_pending = catalog;
                                catalog_refreshed = std::time::Instant::now();
                            } else {
                                let msg = v["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Connection rejected");
                                return Err(msg.to_owned().into());
                            }
                        }
                        if catalog {
                            if v["event"] == "sessions.changed" {
                                catalog_dirty = true;
                            }
                            if v["id"]
                                .as_str()
                                .is_some_and(|id| id.starts_with("sessions.subscribe:"))
                                && v["ok"] != true
                            {
                                emit(Event::Error(
                                    v["error"]["message"]
                                        .as_str()
                                        .unwrap_or("Catalog subscription unavailable; polling")
                                        .into(),
                                ));
                            }
                            if v["id"]
                                .as_str()
                                .is_some_and(|id| id.starts_with("sessions.list:"))
                            {
                                catalog_pending = false;
                                emit(Event::Data(v));
                            }
                        } else {
                            emit(Event::Data(v));
                        }
                    }
                    Ok(Message::Close(_)) => return Err("Gateway closed the connection".into()),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        };
        while !stop.load(Ordering::Relaxed) {
            if let Err(e) = run() {
                emit(Event::Error(e.to_string()));
            }
            for _ in 0..30 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
    Gateway {
        tx: out_tx,
        rx: event_rx,
        cancelled,
    }
}
pub fn rpc(method: &str, params: Value) -> Value {
    request(method, params)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropping_gateway_cancels_its_worker() {
        let (tx, _) = mpsc::channel();
        let (_, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let g = Gateway {
            tx,
            rx,
            cancelled: cancelled.clone(),
        };
        drop(g);
        assert!(cancelled.load(Ordering::Relaxed));
    }
}
