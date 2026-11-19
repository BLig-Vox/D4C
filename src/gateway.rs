mod codex;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::Message;

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
fn request(method: &str, params: Value) -> Value {
    json!({"type":"req","id":format!("{method}:{}",uuid::Uuid::new_v4()),"method":method,"params":params})
}
pub fn connect(url: String, _token: String, ctx: eframe::egui::Context) -> Gateway {
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
                return Err("A secure wss:// Codex server URL is required".into());
            }
            emit(Event::Status("Connecting…".into()));
            let mut socket = crate::workspace::open(&url)?;
            let mut adapter = codex::Adapter::default();
            let send = |socket: &mut crate::workspace::Socket, value: Value| {
                crate::wire::send(socket, Message::Text(value.to_string().into()))
            };
            send(
                &mut socket,
                json!({"id":"initialize","method":"initialize","params":{
                    "clientInfo":{"name":"d4c","title":"D4C","version":env!("CARGO_PKG_VERSION")},
                    "capabilities":{"experimentalApi":true}
                }}),
            )?;
            let mut ready = false;
            let started = Instant::now();
            let mut received = started;
            let mut ping = started;
            loop {
                if stop.load(Ordering::Relaxed) {
                    let _ = socket.close(None);
                    return Ok(());
                }
                if !ready && started.elapsed() >= Duration::from_secs(45) {
                    return Err("Codex initialization timed out".into());
                }
                if received.elapsed() >= Duration::from_secs(90) {
                    return Err("Codex server stopped responding".into());
                }
                if ping.elapsed() >= Duration::from_secs(15) {
                    crate::wire::send(&mut socket, Message::Ping(Vec::new().into()))?;
                    ping = Instant::now();
                }
                if ready {
                    while let Ok(req) = out_rx.try_recv() {
                        adapter.command(req);
                    }
                }
                for output in adapter.output.drain(..) {
                    match output {
                        codex::Output::Wire(v) => send(&mut socket, v)?,
                        codex::Output::Ui(v) => emit(Event::Data(v)),
                    }
                }
                match socket.read() {
                    Ok(msg) => {
                        received = Instant::now();
                        match msg {
                            Message::Text(t) => {
                                let v: Value = serde_json::from_str(&t)?;
                                if !ready && v["id"] == "initialize" {
                                    if !v["result"].is_object() {
                                        return Err(v["error"]["message"]
                                            .as_str()
                                            .unwrap_or("Codex initialization failed")
                                            .to_owned()
                                            .into());
                                    }
                                    send(&mut socket, json!({"method":"initialized"}))?;
                                    ready = true;
                                    emit(Event::Status("Connected".into()));
                                } else {
                                    adapter.incoming(v);
                                }
                            }
                            Message::Close(_) => {
                                return Err("Codex server closed the connection".into());
                            }
                            _ => {}
                        }
                    }
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                        ) => {}
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
