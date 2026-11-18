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
pub struct Runtime {
    pub root: PathBuf,
    units: PathBuf,
    input: Mutex<HashMap<String, Arc<Mutex<Option<rfb::Session>>>>>,
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
impl Runtime {
    pub fn new(root: PathBuf) -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or("HOME missing")?;
        let units = PathBuf::from(home).join(".config/containers/systemd");
        fs::create_dir_all(&units).map_err(|e| e.to_string())?;
        fs::create_dir_all(root.join("desktops")).map_err(|e| e.to_string())?;
        Ok(Self {
            root,
            units,
            input: Mutex::new(HashMap::new()),
        })
    }
    /// Remove legacy desktop RAM limits without restarting its container.
    pub fn remove_memory_limit(&self, d: &Value) -> Result<()> {
        let id = d["id"].as_str().ok_or("Missing desktop ID")?;
        self.prepare(d)?;
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
        let body = r#"{"memory":{"limit":-1,"swap":-1}}"#;
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
                "Podman RAM limit removal failed: {}",
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
        let cpus = d["cpus"].as_u64().unwrap_or(1).clamp(1, 2);
        let home = base.join("home");
        let work = base.join("work");
        let unit = format!(
            "[Unit]\nDescription=D4C desktop {id}\nStartLimitIntervalSec=120\nStartLimitBurst=5\n[Container]\nImage=localhost/d4c-arch-xfce:1\nContainerName=d4c-{id}\nNetwork=pasta:--no-map-gw\nPublishPort=127.0.0.1::5900\nVolume={home}:/home/desktop\nVolume={work}:/workspace\nEnvironment=D4C_WIDTH={width}\nEnvironment=D4C_HEIGHT={height}\nWorkingDir=/workspace\nPodmanArgs=--cpus={cpus} --pids-limit=512\n[Service]\nRestart=on-failure\nRestartSec=3\nTimeoutStartSec=45\n[Install]\nWantedBy=default.target\n",
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
        self.input
            .lock()
            .map_err(|_| "Input lock poisoned")?
            .remove(id);
        Ok(())
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        safe_id(id)?;
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
    pub fn port(&self, id: &str) -> Result<u16> {
        safe_id(id)?;
        let out = command("podman", &["port", &format!("d4c-{id}"), "5900/tcp"])?;
        out.lines()
            .find_map(|s| s.strip_prefix("127.0.0.1:").and_then(|s| s.parse().ok()))
            .ok_or("Desktop is not running".into())
    }
    pub fn status(&self, id: &str) -> Result<String> {
        safe_id(id)?;
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
            *slot = Some(rfb::Session::connect(self.port(id)?).map_err(|e| e.to_string())?);
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
        let mut s = rfb::Session::connect(self.port(id)?).map_err(|e| e.to_string())?;
        let mut d = rfb::Decoder::new(s.width, s.height).map_err(|e| e.to_string())?;
        s.request(false).map_err(|e| e.to_string())?;
        let start = Instant::now();
        let mut b = [0; 65536];
        while start.elapsed() < Duration::from_secs(8) {
            use std::io::Read;
            match s.socket.read(&mut b) {
                Ok(0) => return Err("VNC disconnected".into()),
                Ok(n) => {
                    d.feed(&b[..n]).map_err(|e| e.to_string())?;
                    if d.updates > 0 {
                        return d.png().map_err(|e| e.to_string());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Screenshot timed out".into())
    }
}
