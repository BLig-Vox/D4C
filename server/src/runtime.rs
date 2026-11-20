use crate::{
    rfb,
    store::{Result, Store},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[derive(Default)]
struct ConnectGate {
    failure: Option<(Instant, Duration, String)>,
}
impl ConnectGate {
    fn connect<T>(&mut self, open: impl FnOnce() -> Result<T>) -> Result<T> {
        if let Some((at, delay, reason)) = &self.failure {
            let elapsed = at.elapsed();
            if elapsed < *delay {
                return Err(format!(
                    "{reason}; retry in {}s",
                    (*delay - elapsed).as_secs() + 1
                ));
            }
        }
        match open() {
            Ok(value) => {
                self.failure = None;
                Ok(value)
            }
            Err(reason) => {
                let delay = self
                    .failure
                    .as_ref()
                    .map_or(5, |(_, last, _)| (last.as_secs() * 2).min(60));
                self.failure = Some((Instant::now(), Duration::from_secs(delay), reason.clone()));
                Err(reason)
            }
        }
    }
}
type DesktopSlots<T> = Mutex<HashMap<String, Arc<Mutex<T>>>>;
fn slot<T: Default>(slots: &DesktopSlots<T>, id: &str) -> Result<Arc<Mutex<T>>> {
    safe_id(id)?;
    Ok(slots
        .lock()
        .map_err(|_| "Desktop connections unavailable")?
        .entry(id.to_owned())
        .or_default()
        .clone())
}

struct Capture {
    session: rfb::Session,
    decoder: rfb::Decoder,
}
impl Capture {
    fn frame(&mut self) -> Result<Vec<u8>> {
        let s = &mut self.session;
        let d = &mut self.decoder;
        s.request(false).map_err(|e| e.to_string())?;
        let start = Instant::now();
        let mut buf = [0; 65536];
        while start.elapsed() < Duration::from_secs(8) {
            match s.socket.read(&mut buf) {
                Ok(0) => return Err("VNC disconnected before the screenshot arrived".into()),
                Ok(n) => {
                    let changed = d.feed(&buf[..n]).map_err(|e| e.to_string())?;
                    if (s.width, s.height) != (d.width, d.height) {
                        s.width = d.width;
                        s.height = d.height;
                        s.request(false).map_err(|e| e.to_string())?;
                    } else if changed {
                        return d.png().map_err(|e| e.to_string());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Timed out waiting for a fresh VNC screenshot".into())
    }
}
pub struct Runtime {
    pub root: PathBuf,
    units: PathBuf,
    input: DesktopSlots<Option<rfb::Session>>,
    connections: DesktopSlots<ConnectGate>,
    captures: DesktopSlots<Option<Capture>>,
}
fn command(program: &str, args: &[&str]) -> Result<String> {
    let mut c = Command::new(program);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut p = c.spawn().map_err(|e| format!("{program}: {e}"))?;
    let start = Instant::now();
    // Tools here emit small status outputs; no unbounded build/package logs run through this path.
    loop {
        if p.try_wait().map_err(|e| e.to_string())?.is_some() {
            let out = p.wait_with_output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!(
                    "{program}: {}",
                    String::from_utf8_lossy(&out.stderr)
                        .chars()
                        .take(1000)
                        .collect::<String>()
                ));
            }
            return Ok(String::from_utf8_lossy(&out.stdout).trim().into());
        }
        if start.elapsed() > Duration::from_secs(45) {
            let _ = p.kill();
            let _ = p.wait();
            return Err(format!("{program} timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn safe_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| "Invalid desktop ID".to_string())
        .map(|_| ())
}
fn host_desktop(id: &str) -> bool {
    std::env::var("D4C_HOST_DESKTOP_ID").as_deref() == Ok(id)
}
impl Runtime {
    pub fn new(root: PathBuf) -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or("HOME missing")?;
        let units = PathBuf::from(home).join(".config/containers/systemd");
        fs::create_dir_all(&units).map_err(|e| e.to_string())?;
        fs::create_dir_all(root.join("desktops")).map_err(|e| e.to_string())?;
        Ok(Self {
            root,
            units,
            input: Default::default(),
            connections: Default::default(),
            captures: Default::default(),
        })
    }
    /// Remove legacy desktop CPU and RAM limits without restarting its container.
    pub fn remove_resource_limits(&self, d: &Value) -> Result<()> {
        let id = d["id"].as_str().ok_or("Missing desktop ID")?;
        if host_desktop(id) {
            return Ok(());
        }
        self.prepare(d)?;
        let exists = Command::new("podman")
            .args(["container", "exists", &format!("d4c-{id}")])
            .status()
            .map_err(|e| e.to_string())?;
        if exists.code() == Some(1) {
            return Ok(()); // Stopped Quadlet desktops can have only a unit until next start.
        }
        if !exists.success() {
            return Err("Cannot inspect desktop container".into());
        }
        // The CLI treats --memory=0 as unchanged. The local API accepts OCI's
        // explicit -1 sentinel, updating both persistent configuration and cgroups.
        command("systemctl", &["--user", "start", "podman.socket"])?;
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR missing")?;
        let socket = PathBuf::from(runtime_dir).join("podman/podman.sock");
        let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        let body = r#"{"cpu":{"quota":-1},"memory":{"limit":-1,"swap":-1}}"#;
        write!(stream, "POST /v4.0.0/libpod/containers/d4c-{id}/update HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
        let mut response = String::new();
        stream
            .take(65536)
            .read_to_string(&mut response)
            .map_err(|e| e.to_string())?;
        if !response
            .lines()
            .next()
            .is_some_and(|line| line.split_whitespace().nth(1) == Some("201"))
        {
            return Err(format!(
                "Podman resource limit removal failed: {}",
                response.chars().take(1000).collect::<String>()
            ));
        }
        let inspect: Value =
            serde_json::from_str(&command("podman", &["inspect", &format!("d4c-{id}")])?)
                .map_err(|e| e.to_string())?;
        if inspect[0]["HostConfig"]["Memory"] != -1 || inspect[0]["HostConfig"]["MemorySwap"] != -1
        {
            return Err("Podman did not persist unlimited desktop memory".into());
        }
        if let Some(pid) = inspect[0]["State"]["Pid"].as_u64().filter(|pid| *pid > 0) {
            let membership =
                fs::read_to_string(format!("/proc/{pid}/cgroup")).map_err(|e| e.to_string())?;
            let relative = membership
                .lines()
                .find_map(|line| line.strip_prefix("0::"))
                .ok_or("Desktop cgroup v2 path missing")?;
            let cgroup = PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
            let quota = fs::read_to_string(cgroup.join("cpu.max")).map_err(|e| e.to_string())?;
            if quota.split_whitespace().next() != Some("max") {
                // Older Podman containers retain NanoCpus when updating OCI resources.
                // The regenerated Quadlet handles recreation; lift the live v2 quota
                // without disturbing the desktop process or its current CPU period.
                let period = quota
                    .split_whitespace()
                    .nth(1)
                    .ok_or("Missing CPU period")?;
                let control = cgroup.join("cpu.max");
                fs::write(&control, format!("max {period}")).map_err(|e| e.to_string())?;
                if fs::read_to_string(control)
                    .map_err(|e| e.to_string())?
                    .split_whitespace()
                    .next()
                    != Some("max")
                {
                    return Err("Desktop CPU quota removal failed".into());
                }
            }
            for name in ["memory.max", "memory.swap.max"] {
                if fs::read_to_string(cgroup.join(name))
                    .map_err(|e| e.to_string())?
                    .trim()
                    != "max"
                {
                    return Err(format!("Desktop {name} is still capped"));
                }
            }
        }
        Ok(())
    }
    pub fn prepare(&self, d: &Value) -> Result<()> {
        let id = d["id"].as_str().ok_or("Missing desktop ID")?;
        safe_id(id)?;
        if host_desktop(id) {
            return Ok(());
        }
        command("podman", &["image", "exists", "localhost/d4c-arch-xfce:1"])?;
        let base = self.root.join("desktops").join(id);
        for name in ["home", "work"] {
            let path = base.join(name);
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            command(
                "podman",
                &[
                    "unshare",
                    "chown",
                    "1000:1000",
                    path.to_str().ok_or("Non-UTF8 path")?,
                ],
            )?;
        }
        let width = d["width"].as_u64().unwrap_or(1280).clamp(640, 2560);
        let height = d["height"].as_u64().unwrap_or(800).clamp(480, 1600);
        let mut gpu_devices = String::new();
        let mut render_nodes = Vec::new();
        if let Ok(entries) = fs::read_dir("/dev/dri") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name
                    .to_str()
                    .and_then(|n| n.strip_prefix("renderD"))
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some()
                    && fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(entry.path())
                        .is_ok()
                {
                    render_nodes.push(entry.path());
                }
            }
        }
        render_nodes.sort();
        for node in &render_nodes {
            gpu_devices.push_str(&format!("AddDevice={}\n", node.display()));
        }
        if !render_nodes.is_empty() {
            gpu_devices.push_str("GroupAdd=keep-groups\n");
        }
        let home = base.join("home");
        let work = base.join("work");
        let unit = format!(
            "[Unit]\nDescription=D4C desktop {id}\nStartLimitIntervalSec=120\nStartLimitBurst=5\n[Container]\nImage=localhost/d4c-arch-xfce:1\nContainerName=d4c-{id}\nNetwork=pasta:--no-map-gw\nPublishPort=127.0.0.1::5900\nVolume={home}:/home/desktop\nVolume={work}:/workspace\nEnvironment=D4C_WIDTH={width}\nEnvironment=D4C_HEIGHT={height}\nWorkingDir=/workspace\n{gpu_devices}PodmanArgs=--pids-limit=512\n[Service]\nRestart=on-failure\nRestartSec=3\nTimeoutStartSec=45\n[Install]\nWantedBy=default.target\n",
            home = home.display(),
            work = work.display()
        );
        let unit = if d["desired"] == "running" {
            unit
        } else {
            unit.replace("[Install]\nWantedBy=default.target\n", "")
        };
        fs::write(self.units.join(format!("d4c-{id}.container")), unit)
            .map_err(|e| e.to_string())?;
        command("systemctl", &["--user", "daemon-reload"])?;
        Ok(())
    }
    pub fn start(&self, id: &str) -> Result<()> {
        safe_id(id)?;
        if self.status(id).is_ok_and(|status| status == "running") {
            return Ok(());
        }
        let available = fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u64>().ok())
        });
        if available.is_some_and(|kb| kb < 2 * 1024 * 1024) {
            return Err("Not enough server memory to start another desktop".into());
        }
        command(
            "systemctl",
            &["--user", "reset-failed", &format!("d4c-{id}.service")],
        )?;
        command(
            "systemctl",
            &["--user", "start", &format!("d4c-{id}.service")],
        )?;
        Ok(())
    }
    pub fn stop(&self, id: &str) -> Result<()> {
        safe_id(id)?;
        command(
            "systemctl",
            &["--user", "stop", &format!("d4c-{id}.service")],
        )?;
        self.captures
            .lock()
            .map_err(|_| "Capture lock unavailable")?
            .remove(id);
        self.connections
            .lock()
            .map_err(|_| "Connection lock unavailable")?
            .remove(id);
        self.input
            .lock()
            .map_err(|_| "Input lock poisoned")?
            .remove(id);
        Ok(())
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        safe_id(id)?;
        if host_desktop(id) {
            return self.stop(id);
        }
        let unit = self.units.join(format!("d4c-{id}.container"));
        if unit.exists() {
            self.stop(id)?;
        }
        command(
            "podman",
            &["rm", "--force", "--ignore", &format!("d4c-{id}")],
        )?;
        if unit.exists() {
            fs::remove_file(unit).map_err(|e| e.to_string())?;
        }
        command("systemctl", &["--user", "daemon-reload"])?;
        // Persistent home/work files are deliberately retained. A separate explicit purge is required.
        Ok(())
    }
    pub fn connect_vnc(&self, id: &str) -> Result<rfb::Session> {
        let gate = slot(&self.connections, id)?;
        let mut gate = gate.lock().map_err(|_| "VNC connection gate unavailable")?;
        gate.connect(|| {
            if host_desktop(id) {
                return rfb::Session::connect_local(&self.root.join("host-screen.sock"))
                    .map_err(|e| format!("Host screen: {e}"));
            }
            rfb::Session::connect(self.port(id)?).map_err(|e| {
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) {
                    "VNC setup timed out waiting for the desktop".into()
                } else {
                    format!("VNC setup: {e}")
                }
            })
        })
    }
    fn port(&self, id: &str) -> Result<u16> {
        safe_id(id)?;
        let out = command("podman", &["port", &format!("d4c-{id}"), "5900/tcp"])?;
        out.lines()
            .find_map(|s| s.strip_prefix("127.0.0.1:").and_then(|s| s.parse().ok()))
            .ok_or("Desktop is not running".into())
    }
    pub fn status(&self, id: &str) -> Result<String> {
        safe_id(id)?;
        if host_desktop(id) {
            let running = command(
                "systemctl",
                &["--user", "is-active", &format!("d4c-{id}.service")],
            )
            .is_ok();
            return Ok(if running { "running" } else { "stopped" }.into());
        }
        let result = command(
            "podman",
            &[
                "inspect",
                "--format",
                "{{.State.Status}}",
                &format!("d4c-{id}"),
            ],
        );
        Ok(match result {
            Ok(s) if s == "running" => "running",
            Ok(_) => "stopped",
            Err(_) => "stopped",
        }
        .into())
    }
    pub fn input(
        &self,
        store: &Arc<Mutex<Store>>,
        id: &str,
        client: Option<&str>,
        bytes: &[u8],
    ) -> Result<()> {
        safe_id(id)?;
        if bytes.len() > 131072 {
            return Err("Input batch too large".into());
        }
        let slot = {
            let mut map = self.input.lock().map_err(|_| "Input map poisoned")?;
            map.entry(id.into())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut slot = slot.lock().map_err(|_| "Input session poisoned")?;
        if slot.is_none() {
            *slot = Some(self.connect_vnc(id)?);
        }
        // Re-check the authoritative control mode immediately before dispatch, after network setup.
        let db = store.lock().map_err(|_| "Database lock poisoned")?;
        if let Some(client) = client {
            db.human_input(id, client, crate::store::now())?;
        } else {
            db.agent_input(id)?;
        }
        let result = slot
            .as_mut()
            .unwrap()
            .socket
            .write_all(bytes)
            .map_err(|e| e.to_string());
        drop(db);
        if result.is_err() {
            *slot = None;
        }
        result
    }
    pub fn reset_input(&self, id: &str) -> Result<()> {
        let slot = self
            .input
            .lock()
            .map_err(|_| "Input map unavailable")?
            .remove(id);
        if let Some(slot) = slot {
            let mut slot = slot.lock().map_err(|_| "Input session unavailable")?;
            if let Some(session) = slot.as_mut() {
                let mut releases = Vec::new();
                for key in (0..=255).chain(0xff00..=0xffff) {
                    releases.extend(rfb::key_event(key, false));
                }
                releases.extend(rfb::pointer_event(0, 0, 0));
                let _ = session.socket.write_all(&releases);
            }
            *slot = None;
        }
        Ok(())
    }
    pub fn screenshot(&self, id: &str) -> Result<Vec<u8>> {
        let capture = slot(&self.captures, id)?;
        let mut capture = capture.lock().map_err(|_| "Capture unavailable")?;
        // Reuse the negotiated stream and its zlib state. A stale stream can be
        // replaced once because a read-only screenshot is safe to request again.
        let mut retry = capture.is_some();
        loop {
            if capture.is_none() {
                let session = self.connect_vnc(id)?;
                let decoder =
                    rfb::Decoder::new(session.width, session.height).map_err(|e| e.to_string())?;
                *capture = Some(Capture { session, decoder });
            }
            let result = capture.as_mut().unwrap().frame();
            if result.is_ok() {
                return result;
            }
            *capture = None;
            if !retry {
                return result;
            }
            retry = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_handshakes_share_a_cooldown_and_success_resets_it() {
        let mut gate = ConnectGate::default();
        assert!(gate.connect::<()>(|| Err("busy".into())).is_err());
        assert!(gate.connect::<()>(|| panic!("repeated handshake")).is_err());
        gate.failure.as_mut().unwrap().0 -= Duration::from_secs(6);
        assert!(gate.connect::<()>(|| Err("still busy".into())).is_err());
        assert_eq!(gate.failure.as_ref().unwrap().1, Duration::from_secs(10));
        gate.failure.as_mut().unwrap().0 -= Duration::from_secs(11);
        assert!(gate.connect(|| Ok(())).is_ok());
        assert!(gate.failure.is_none());
    }

    #[test]
    fn sequential_screenshots_request_and_decode_fresh_frames_on_one_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let peer = std::thread::spawn(move || {
            for color in [32, 127] {
                let mut request = [0; 10];
                server.read_exact(&mut request).unwrap();
                assert_eq!(request, [3, 0, 0, 0, 0, 0, 0, 1, 0, 1]);
                server
                    .write_all(&[
                        0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, color, 0, 0, 0,
                    ])
                    .unwrap();
            }
        });
        let mut capture = Capture {
            session: rfb::Session {
                socket: Box::new(client),
                width: 1,
                height: 1,
                name: String::new(),
            },
            decoder: rfb::Decoder::new(1, 1).unwrap(),
        };
        assert_ne!(capture.frame().unwrap(), capture.frame().unwrap());
        assert!(capture.frame().is_err());
        peer.join().unwrap();
    }
}
