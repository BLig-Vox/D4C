use crate::{
    auth,
    manager::{Actor, Manager},
    store::Result,
};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};
type Socket = WebSocket<StreamOwned<ServerConnection, TcpStream>>;
struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
fn permit(active: &Arc<AtomicUsize>) -> Option<Permit> {
    let old = active.fetch_add(1, Ordering::Relaxed);
    if old >= 32 {
        active.fetch_sub(1, Ordering::Relaxed);
        None
    } else {
        Some(Permit(active.clone()))
    }
}
pub fn serve(manager: Manager, config: &Value) -> Result<()> {
    let host = config["hostname"]
        .as_str()
        .ok_or("hostname missing")?
        .to_string();
    let address: std::net::SocketAddr = config["listen"]
        .as_str()
        .ok_or("listen missing")?
        .parse()
        .map_err(|_| "Invalid listen address")?;
    let allowed = match address.ip() {
        std::net::IpAddr::V4(ip) => {
            ip.is_loopback() || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        }
        std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.segments()[0] == 0xfd7a,
    };
    if !allowed {
        return Err("Bind only a tailnet or loopback address".into());
    }
    let cert = fs::File::open(
        config["certificate"]
            .as_str()
            .ok_or("certificate missing")?,
    )
    .map_err(|e| e.to_string())?;
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let key = fs::File::open(config["privateKey"].as_str().ok_or("privateKey missing")?)
        .map_err(|e| e.to_string())?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key))
        .map_err(|e| e.to_string())?
        .ok_or("Private key missing")?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| e.to_string())?,
    );
    let active = Arc::new(AtomicUsize::new(0));
    let socket = config["socket"]
        .as_str()
        .unwrap_or("/run/d4c/manager.sock")
        .to_string();
    let (ipc_listener, _ipc_lock) = bind_ipc(&socket)?;
    let observer_uid = config["observerUid"].as_u64().unwrap_or(0);
    let observer_uid = u32::try_from(observer_uid).map_err(|_| "Invalid observerUid")?;
    if config["socketGroupAccess"] == true {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o660))
            .map_err(|e| e.to_string())?;
    }
    let ipc = manager.clone();
    let ipc_active = active.clone();
    std::thread::spawn(move || {
        if let Err(e) = serve_ipc(ipc, ipc_listener, ipc_active, observer_uid) {
            eprintln!("IPC failed: {e}");
        }
    });
    let codex_socket = config["codexSocket"].as_str().map(std::path::PathBuf::from);
    if codex_socket.as_ref().is_some_and(|p| !p.is_absolute()) {
        return Err("codexSocket requires an absolute Unix socket path".into());
    }
    let listener = TcpListener::bind(address).map_err(|e| e.to_string())?;
    manager.reconcile();
    let watcher = manager.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(4));
            let all = watcher
                .db
                .lock()
                .ok()
                .and_then(|s| s.list("desktop").ok())
                .unwrap_or_default();
            for d in all.into_iter().filter(|d| {
                d["deleted"] != true && matches!(d["status"].as_str(), Some("running" | "stopped"))
            }) {
                let Some(id) = d["id"].as_str() else {
                    continue;
                };
                if d["mode"] == "human"
                    && d["leaseUntil"].as_u64().unwrap_or(0) <= crate::store::now()
                {
                    let _ = watcher.runtime.reset_input(id);
                }
                let Ok(status) = watcher.runtime.status(id) else {
                    continue;
                };
                if d["status"] != status {
                    if let Ok(mut db) = watcher.db.lock() {
                        if let Ok(mut current) = db.get("desktop", id) {
                            if current["rev"] == d["rev"] {
                                let rev = current["rev"].as_u64().unwrap_or(0);
                                current["status"] = json!(status);
                                let _ = db.put("desktop", id, Some(rev), current);
                            }
                        }
                    }
                }
            }
        }
    });
    eprintln!("D4C listening on {address}");
    for connection in listener.incoming() {
        let tcp = connection.map_err(|e| e.to_string())?;
        let Some(guard) = permit(&active) else {
            continue;
        };
        let peer = tcp.peer_addr().ok();
        let tls = tls.clone();
        let manager = manager.clone();
        let host = host.clone();
        let codex_socket = codex_socket.clone();
        std::thread::spawn(move || {
            let _guard = guard;
            if let Err(e) = connection_task(tcp, tls, manager, &host, codex_socket.as_deref()) {
                eprintln!("Connection ended peer={peer:?}: {e}");
            }
        });
    }
    Ok(())
}
fn send(socket: &mut Socket, value: Value) -> Result<()> {
    crate::wire::send(socket, Message::Text(value.to_string().into())).map_err(|e| e.to_string())
}
fn connection_task(
    tcp: TcpStream,
    tls: Arc<ServerConfig>,
    manager: Manager,
    host: &str,
    codex_socket: Option<&Path>,
) -> Result<()> {
    tcp.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    let stream = StreamOwned::new(ServerConnection::new(tls).map_err(|e| e.to_string())?, tcp);
    let mut path = String::new();
    let mut compressed = false;
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024));
    let mut socket = tungstenite::accept_hdr_with_config(
        stream,
        |req: &tungstenite::handshake::server::Request, response| {
            path = req.uri().path().to_string();
            compressed = req.uri().query().is_some_and(|q| {
                q.split('&')
                    .any(|part| part == format!("transport={}", crate::desktop_wire::NAME))
            });
            let valid_origin = req.headers().get("origin").is_none_or(|v| {
                v.to_str().is_ok_and(|o| {
                    o == format!("https://{host}")
                        || o == format!("https://{host}:8443")
                        || o == format!("https://{host}:8444")
                })
            });
            if !valid_origin {
                return Err(tungstenite::http::Response::builder()
                    .status(403)
                    .body(Some("Origin rejected".into()))
                    .unwrap());
            }
            Ok(response)
        },
        Some(ws_config),
    )
    .map_err(|e| format!("WebSocket handshake: {e}"))?;
    let nonce = auth::nonce();
    send(
        &mut socket,
        json!({"type":"challenge","hostname":host,"nonce":nonce}),
    )?;
    let proof = match socket.read().map_err(|e| e.to_string())? {
        Message::Text(s) => serde_json::from_str(&s).map_err(|e| e.to_string())?,
        _ => return Err("Authentication proof required".into()),
    };
    let (id, public) = auth::verify(host, &nonce, &proof)?;
    let approved = manager
        .db
        .lock()
        .map_err(|_| "Database unavailable")?
        .request_device(&id, &public)?;
    if !approved {
        send(
            &mut socket,
            json!({"type":"pairing_required","deviceId":id}),
        )?;
        return Ok(());
    }
    send(&mut socket, json!({"type":"authenticated","deviceId":id}))?;
    socket
        .get_mut()
        .sock
        .set_read_timeout(Some(Duration::from_millis(150)))
        .map_err(|e| e.to_string())?;
    socket
        .get_mut()
        .sock
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(|e| e.to_string())?;
    if path == "/control" {
        control(socket, manager, id)
    } else if path == "/codex" || path == "/" {
        codex(
            socket,
            codex_socket.ok_or("Direct Codex is not configured")?,
            &manager,
        )
    } else if let Some(desktop) = path.strip_prefix("/vnc/") {
        uuid::Uuid::parse_str(desktop).map_err(|_| "Invalid desktop ID")?;
        viewer(socket, manager, desktop, compressed)
    } else {
        Err("Unknown endpoint".into())
    }
}
struct Heartbeat {
    received: Instant,
    sent: Instant,
}
impl Heartbeat {
    fn new() -> Self {
        Self {
            received: Instant::now(),
            sent: Instant::now(),
        }
    }
    fn poll(&mut self, socket: &mut Socket) -> Result<()> {
        if self.received.elapsed() >= Duration::from_secs(90) {
            return Err("WebSocket peer stopped responding".into());
        }
        if self.sent.elapsed() >= Duration::from_secs(15) {
            crate::wire::send(socket, Message::Ping(Vec::new().into()))
                .map_err(|e| e.to_string())?;
            self.sent = Instant::now();
        }
        Ok(())
    }
}
/// One upstream WebSocket per paired client. The Codex service owns the threads
/// and turns, so leaving the app only closes this subscription.
fn codex(mut client: Socket, path: &Path, manager: &Manager) -> Result<()> {
    use std::os::unix::net::UnixStream;
    let stream = UnixStream::connect(path).map_err(|e| format!("Codex socket: {e}"))?;
    let timeout = |e: io::Error| e.to_string();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(timeout)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(timeout)?;
    let (mut upstream, _) = tungstenite::client("ws://localhost/", stream)
        .map_err(|e| format!("Codex handshake: {e}"))?;
    // Native Codex emits a whole JSON response in one frame. Allow a single
    // large turn up to the existing message limit on this local Unix socket.
    upstream.set_config(|cfg| cfg.max_frame_size = cfg.max_message_size);
    let tick = Some(Duration::from_millis(10));
    upstream.get_mut().set_read_timeout(tick).map_err(timeout)?;
    client
        .get_mut()
        .sock
        .set_read_timeout(tick)
        .map_err(timeout)?;
    client.set_config(|cfg| {
        cfg.max_frame_size = Some(16 << 20);
        cfg.max_message_size = Some(16 << 20);
    });
    let mut heartbeat = Heartbeat::new();
    loop {
        heartbeat.poll(&mut client)?;
        match receive_codex(&mut client)? {
            Some(Message::Close(_)) => return Ok(()),
            Some(message) => {
                heartbeat.received = Instant::now();
                if message.is_text() {
                    if let Ok(mut request) =
                        serde_json::from_str::<Value>(message.to_text().unwrap_or(""))
                    {
                        if let Some(action) = request["method"]
                            .as_str()
                            .and_then(|s| s.strip_prefix("d4c/cron/"))
                        {
                            let result = crate::schedules::rpc(action, &request["params"]);
                            let response = match result {
                                Ok(v) => json!({"id":request["id"],"result":v}),
                                Err(e) => {
                                    json!({"id":request["id"],"error":{"code":-32602,"message":e}})
                                }
                            };
                            send(&mut client, response)?;
                            continue;
                        }
                        if request["method"] == "thread/turns/list"
                            && request["params"]["itemsView"] == "full"
                        {
                            // A single desktop-heavy turn can contain megabytes of screenshots.
                            // Codex returns a cursor for the remaining turns.
                            request["params"]["limit"] = json!(1);
                            send_native(&mut upstream, request)?;
                            continue;
                        }
                        if request["method"] == "thread/start" {
                            crate::codex_tools::augment_thread_start(&mut request);
                            send_native(&mut upstream, request)?;
                            continue;
                        }
                        if request["method"] == "d4c/attachment/store" {
                            let response = match store_attachment(
                                &manager.runtime.root.join("uploads"),
                                &request["params"],
                            ) {
                                Ok(value) => json!({"id":request["id"],"result":value}),
                                Err(error) => {
                                    json!({"id":request["id"],"error":{"code":-32602,"message":error}})
                                }
                            };
                            send(&mut client, response)?;
                            continue;
                        }
                    }
                    crate::wire::send(&mut upstream, message).map_err(|e| e.to_string())?;
                }
            }
            None => {}
        }
        match receive_codex(&mut upstream)? {
            Some(Message::Close(_)) => return Ok(()),
            Some(message) if message.is_text() => {
                if let Ok(mut req) = serde_json::from_str::<Value>(message.to_text().unwrap_or(""))
                {
                    if compact_history_images(&mut req) {
                        send(&mut client, req)?;
                        continue;
                    }
                    if let Some(reply) = crate::codex_tools::answer(manager, &req) {
                        send_native(&mut upstream, reply)?;
                        continue;
                    }
                }
                crate::wire::send(&mut client, message).map_err(|e| e.to_string())?;
            }
            _ => {}
        }
    }
}

// The transcript displays tool results as text. Avoid transferring base64 screenshots
// as JSON text while retaining the original result in Codex and all user attachments.
fn compact_history_images(response: &mut Value) -> bool {
    let mut changed = false;
    let Some(turns) = response
        .pointer_mut("/result/data")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    for turn in turns {
        for item in turn
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if item["type"] != "dynamicToolCall" {
                continue;
            }
            for part in item
                .get_mut("contentItems")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if part["type"] == "inputImage"
                    && part["imageUrl"]
                        .as_str()
                        .is_some_and(|url| url.starts_with("data:image/"))
                {
                    *part =
                        json!({"type":"inputText","text":"[Tool image saved in Codex history]"});
                    changed = true;
                }
            }
        }
    }
    changed
}

fn send_native<S: Read + Write>(ws: &mut WebSocket<S>, value: Value) -> Result<()> {
    crate::wire::send(ws, Message::Text(value.to_string().into())).map_err(|e| e.to_string())
}

fn store_attachment(root: &Path, p: &Value) -> Result<Value> {
    use base64::Engine as _;
    use std::os::unix::fs::OpenOptionsExt;
    let name = p["name"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 240
                && !s.chars().any(|c| c.is_control() || c == '/' || c == '\\')
                && *s != "."
                && *s != ".."
        })
        .ok_or("Invalid attachment filename")?;
    let encoded = p["data"]
        .as_str()
        .filter(|s| s.len() <= 8_000_000)
        .ok_or("File exceeds 6 MB")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Invalid file encoding")?;
    let dir = root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(dir);
        return Err(error.to_string());
    }
    Ok(json!({"path":path}))
}

fn receive_codex<S: Read + Write>(socket: &mut WebSocket<S>) -> Result<Option<Message>> {
    match socket.read() {
        Ok(message) => Ok(Some(message)),
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn control(mut socket: Socket, manager: Manager, client: String) -> Result<()> {
    let snapshot = manager
        .db
        .lock()
        .map_err(|_| "Database unavailable")?
        .snapshot()?;
    let mut cursor = snapshot["revision"].as_u64().unwrap_or(0);
    send(&mut socket, json!({"type":"snapshot","payload":snapshot}))?;
    let mut heartbeat = Heartbeat::new();
    loop {
        heartbeat.poll(&mut socket)?;
        let incoming = socket.read();
        if incoming.is_ok() {
            heartbeat.received = Instant::now();
        }
        match incoming {
            Ok(Message::Text(text)) => {
                let request: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                let method = request["method"].as_str().ok_or("method missing")?;
                let result =
                    manager.dispatch(&Actor::Human(client.clone()), method, &request["params"]);
                match result {
                    Ok(payload) => send(
                        &mut socket,
                        json!({"type":"response","id":request["id"],"ok":true,"payload":payload}),
                    )?,
                    Err(error) => send(
                        &mut socket,
                        json!({"type":"response","id":request["id"],"ok":false,"error":error}),
                    )?,
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
        let changes = {
            let db = manager.db.lock().map_err(|_| "Database unavailable")?;
            if db.revision()? > cursor {
                Some(db.changes(cursor)?)
            } else {
                None
            }
        };
        if let Some(changes) = changes {
            cursor = changes["revision"]
                .as_u64()
                .or_else(|| changes["snapshot"]["revision"].as_u64())
                .unwrap_or(cursor);
            send(&mut socket, json!({"type":"changes","payload":changes}))?;
        }
    }
}
fn viewer(mut socket: Socket, manager: Manager, id: &str, compressed: bool) -> Result<()> {
    socket
        .get_mut()
        .sock
        .set_read_timeout(Some(Duration::from_millis(1)))
        .map_err(|e| e.to_string())?;
    let mut session = manager.runtime.connect_vnc(id)?;
    send(
        &mut socket,
        json!({"type":"framebuffer","width":session.width,"height":session.height,"name":session.name,"encoding":"rfb-bgrx32","transport":if compressed {crate::desktop_wire::NAME}else{"raw"}}),
    )?;
    session
        .socket
        .set_read_timeout(Some(Duration::from_millis(20)))
        .map_err(|e| e.to_string())?;
    session.request(false).map_err(|e| e.to_string())?;
    let mut refresh = Instant::now();
    let mut bytes = [0; 65536];
    let mut heartbeat = Heartbeat::new();
    loop {
        if refresh.elapsed() > Duration::from_millis(100) {
            session.request(true).map_err(|e| e.to_string())?;
            refresh = Instant::now();
        }
        match session.socket.read(&mut bytes) {
            Ok(0) => return Err("VNC disconnected".into()),
            Ok(n) => {
                let packet = if compressed {
                    crate::desktop_wire::encode(&bytes[..n]).map_err(|e| e.to_string())?
                } else {
                    bytes[..n].to_vec()
                };
                crate::wire::send(&mut socket, Message::Binary(packet.into()))
                    .map_err(|e| e.to_string())?;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
        // The framebuffer channel never forwards client bytes into VNC. All input goes
        // through typed, server-authorized control RPCs, so takeover cannot be bypassed.
        heartbeat.poll(&mut socket)?;
        let incoming = socket.read();
        if incoming.is_ok() {
            heartbeat.received = Instant::now();
        }
        match incoming {
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}
#[cfg(unix)]
fn bind_ipc(path: &str) -> Result<(std::os::unix::net::UnixListener, fs::File)> {
    use std::os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixListener,
    };
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let path = Path::new(path);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.with_extension("sock.lock"))
        .map_err(|e| e.to_string())?;
    // Hold the advisory lock for the listener's entire lifetime. A second
    // invocation must never unlink the running manager's socket.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Another manager owns the IPC socket".into());
    }
    if path.exists() {
        if !fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_socket()
        {
            return Err("IPC path is not a socket".into());
        }
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => return Err("An active manager is already listening on the IPC socket".into()),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) => {}
            Err(e) => return Err(format!("Cannot establish whether IPC socket is stale: {e}")),
        }
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    let listener = UnixListener::bind(path).map_err(|e| e.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok((listener, lock))
}
#[cfg(unix)]
fn serve_ipc(
    manager: Manager,
    listener: std::os::unix::net::UnixListener,
    active: Arc<AtomicUsize>,
    observer_uid: u32,
) -> Result<()> {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let Some(guard) = permit(&active) else {
            continue;
        };
        let manager = manager.clone();
        std::thread::spawn(move || {
            let _guard = guard;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let mut line = String::new();
            if BufReader::new((&stream).take(1024 * 1024))
                .read_line(&mut line)
                .is_err()
            {
                return;
            }
            let result = (|| {
                let req: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
                let session = req["session"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 512)
                    .ok_or("Conversation scope required")?;
                if req["method"] == "observer.event" {
                    if !peer_is_owner(&stream, observer_uid)? {
                        return Err("Observer ingress requires the OpenClaw service owner".into());
                    }
                    return crate::observer::record(&manager, session, &req["params"]);
                }
                manager.dispatch(
                    &Actor::Agent(session.into()),
                    req["method"].as_str().ok_or("method required")?,
                    &req["params"],
                )
            })();
            let value = match result {
                Ok(payload) => json!({"ok":true,"payload":payload}),
                Err(error) => json!({"ok":false,"error":error}),
            };
            let _ = writeln!(stream, "{value}");
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_is_owner(stream: &std::os::unix::net::UnixStream, owner_uid: u32) -> Result<bool> {
    use std::os::fd::AsRawFd;
    let mut credentials = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SO_PEERCRED copies this fixed-size Linux structure into the owned buffer.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return Err("Cannot verify observer peer".into());
    }
    Ok(credentials.uid == owner_uid)
}

#[cfg(test)]
mod ipc_tests {
    use super::*;
    #[test]
    fn history_compacts_only_embedded_tool_images() {
        let image = json!({"type":"inputImage","imageUrl":"data:image/png;base64,test"});
        let user = json!({"type":"userMessage","content":[image.clone()]});
        let mut response = json!({"result":{"data":[{"items":[user.clone(),
            {"type":"dynamicToolCall","contentItems":[image,{"type":"inputText","text":"ok"}]}
        ]}],"nextCursor":"older"}});
        assert!(compact_history_images(&mut response));
        assert_eq!(response["result"]["nextCursor"], "older");
        let items = &response["result"]["data"][0]["items"];
        assert_eq!(items[0], user);
        assert_eq!(items[1]["contentItems"][0]["type"], "inputText");
        assert_eq!(items[1]["contentItems"][1]["text"], "ok");
        assert!(!compact_history_images(&mut response));
    }
    #[test]
    fn second_manager_cannot_unlink_live_socket() {
        let dir = std::env::temp_dir().join(format!("d4c-ipc-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("manager.sock");
        let path_str = path.to_str().unwrap();
        let (listener, guard) = bind_ipc(path_str).unwrap();
        assert!(bind_ipc(path_str).is_err());
        assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
        drop(listener);
        drop(guard);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn dead_socket_is_recovered_after_old_owner_exits() {
        let dir = std::env::temp_dir().join(format!("d4c-ipc-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("manager.sock");
        let path_str = path.to_str().unwrap();
        let old = bind_ipc(path_str).unwrap();
        drop(old);
        let new = bind_ipc(path_str).unwrap();
        assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
        drop(new);
        fs::remove_dir_all(dir).unwrap();
    }
}
