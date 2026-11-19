use crate::{
    auth,
    manager::{Actor, Manager},
    rfb,
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
        std::thread::spawn(move || {
            let _guard = guard;
            if let Err(e) = connection_task(tcp, tls, manager, &host) {
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
                    o == format!("https://{host}") || o == format!("https://{host}:8443")
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
    let mut session =
        rfb::Session::connect(manager.runtime.port(id)?).map_err(|e| e.to_string())?;
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
