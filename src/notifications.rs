use std::{
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};
struct Alert {
    title: String,
    body: String,
    desktop: bool,
    sound: bool,
}
pub struct Notifier {
    tx: SyncSender<Alert>,
    rx: Receiver<String>,
    pub status: String,
}
impl Default for Notifier {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel::<Alert>(8);
        let (errors, reports) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(a) = rx.recv() {
                let mut errors_out = Vec::new();
                if a.desktop {
                    if let Err(e) = desktop(&a.title, &a.body) {
                        errors_out.push(e);
                    }
                }
                if a.sound {
                    if let Err(e) = sound() {
                        errors_out.push(e);
                    }
                }
                let _ = errors.send(if errors_out.is_empty() {
                    "Alert delivered to the local OS".into()
                } else {
                    errors_out.join(" · ")
                });
            }
        });
        Self {
            tx,
            rx: reports,
            status: "Alerts use the OS running D4C".into(),
        }
    }
}
impl Notifier {
    #[cfg(not(target_os = "android"))]
    pub fn send(&mut self, title: &str, body: &str, desktop: bool, sound: bool) {
        if !(desktop || sound) {
            return;
        }
        if self
            .tx
            .try_send(Alert {
                title: title.chars().take(120).collect(),
                body: body.chars().take(280).collect(),
                desktop,
                sound,
            })
            .is_err()
        {
            self.status = "Alert queue full; results remain in Notifications".into();
        }
    }
    #[cfg(target_os = "android")]
    pub fn send(&mut self, _title: &str, _body: &str, _desktop: bool, _sound: bool) {
        // The independent service delivers durable server alerts, including this chat.
        // UI catch-up must not issue a second notification for the same reply.
        self.status = "Background alerts are delivered by the Android connection service".into();
    }
    pub fn test(&mut self, desktop: bool, sound: bool) {
        #[cfg(target_os = "android")]
        {
            if !desktop && !sound {
                return;
            }
            self.status = match crate::android_notifications::alert(
                "D4C",
                "Notifications are ready",
                sound,
            ) {
                Ok(()) => "Android notification requested; delivery follows system settings".into(),
                Err(e) => e,
            };
        }
        #[cfg(not(target_os = "android"))]
        self.send("D4C", "Notifications are ready", desktop, sound);
    }
    pub fn poll(&mut self) {
        for status in self.rx.try_iter() {
            self.status = status;
        }
    }
}
fn run(cmd: &mut Command) -> Result<(), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                return if s.success() {
                    Ok(())
                } else {
                    Err(format!("OS alert command exited {s}"))
                };
            }
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
        if start.elapsed() > Duration::from_secs(6) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("OS alert service did not respond".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}
#[cfg(target_os = "linux")]
fn desktop(title: &str, body: &str) -> Result<(), String> {
    run(Command::new("notify-send").args([
        "--app-name=D4C",
        "--icon=dialog-information",
        "--expire-time=8000",
        "--",
        title,
        body,
    ]))
    .map_err(|e| {
        format!(
            "Desktop alerts unavailable: {e}. Install libnotify-bin and run a notification service."
        )
    })
}
#[cfg(target_os = "macos")]
fn desktop(title: &str, body: &str) -> Result<(), String> {
    run(Command::new("osascript").args([
        "-e",
        "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run",
        "--",
        title,
        body,
    ]))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn desktop(_title: &str, _body: &str) -> Result<(), String> {
    Err("Native alerts are currently supported on Linux and macOS; use the in-app bell here".into())
}
#[cfg(target_os = "linux")]
fn sound() -> Result<(), String> {
    for path in [
        "/usr/share/sounds/freedesktop/stereo/message.oga",
        "/usr/share/sounds/freedesktop/stereo/complete.oga",
    ] {
        if std::path::Path::new(path).exists() {
            return run(Command::new("paplay").arg(path))
                .map_err(|e| format!("Sound unavailable: {e}"));
        }
    }
    Err(
        "Notification sound unavailable: install sound-theme-freedesktop and pulseaudio-utils"
            .into(),
    )
}
#[cfg(target_os = "macos")]
fn sound() -> Result<(), String> {
    run(Command::new("afplay").arg("/System/Library/Sounds/Pop.aiff"))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sound() -> Result<(), String> {
    Err("Notification sound is not yet supported on this OS".into())
}
