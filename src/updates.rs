use eframe::egui;
use semver::Version;
use serde_json::Value;
use std::{sync::mpsc, time::Duration};

const REPOSITORY: &str = "BLig-Vox/D4C";

pub(super) struct Release {
    pub version: Version,
    pub download: String,
}

#[derive(Default)]
pub(super) enum Check {
    #[default]
    Idle,
    Pending(mpsc::Receiver<Result<Option<Release>, String>>),
    Finished(Result<Option<Release>, String>),
}

impl Check {
    pub fn start(&mut self, alpha: bool, ctx: &egui::Context) {
        let (send, receive) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("d4c-update".into())
            .spawn(move || {
                let _ = send.send(fetch(alpha));
                ctx.request_repaint();
            }) {
            Ok(_) => *self = Self::Pending(receive),
            Err(error) => *self = Self::Finished(Err(error.to_string())),
        }
    }

    pub fn poll(&mut self) {
        if let Self::Pending(receiver) = self {
            match receiver.try_recv() {
                Ok(result) => *self = Self::Finished(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    *self = Self::Finished(Err("Update check ended unexpectedly".into()));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}

fn asset_name(version: &Version, os: &str, arch: &str) -> Option<String> {
    match (os, arch) {
        ("windows", "x86_64") => Some(format!("d4c-{version}-windows-x86_64.zip")),
        ("linux", "x86_64") => Some(format!("d4c-{version}-linux-x86_64.tar.gz")),
        _ => None,
    }
}

fn latest(releases: &[Value], alpha: bool, os: &str, arch: &str) -> Option<Release> {
    releases
        .iter()
        .filter_map(|entry| {
            if entry["draft"].as_bool() != Some(false) {
                return None;
            }
            let version = Version::parse(entry["tag_name"].as_str()?.strip_prefix('v')?).ok()?;
            if !alpha && (entry["prerelease"].as_bool() != Some(false) || !version.pre.is_empty()) {
                return None;
            }
            let name = asset_name(&version, os, arch)?;
            if !entry["assets"]
                .as_array()?
                .iter()
                .any(|asset| asset["name"].as_str() == Some(&name))
            {
                return None;
            }
            let download =
                format!("https://github.com/{REPOSITORY}/releases/download/v{version}/{name}");
            Some(Release { version, download })
        })
        .max_by(|a, b| a.version.cmp_precedence(&b.version))
}

fn fetch(alpha: bool) -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .build()
        .into();
    let body = agent
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases?per_page=100"
        ))
        .header("User-Agent", concat!("D4C/", env!("CARGO_PKG_VERSION")))
        .call()
        .and_then(|mut response| {
            response
                .body_mut()
                .with_config()
                .limit(8 * 1024 * 1024)
                .read_to_string()
        })
        .map_err(|error| format!("GitHub update check failed ({error})"))?;
    let releases: Vec<Value> = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    Ok(latest(
        &releases,
        alpha,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(version: &str, flags: (bool, bool), assets: &[&str]) -> Value {
        json!({
            "tag_name": format!("v{version}"), "draft": flags.0, "prerelease": flags.1,
            "assets": assets.iter().map(|asset| json!({"name": asset})).collect::<Vec<_>>()
        })
    }

    #[test]
    fn channel_and_platform_filter_before_selecting_highest_version() {
        let entries = [
            release("0.2.0", (true, false), &["d4c-0.2.0-windows-x86_64.zip"]),
            release(
                "0.1.2-alpha.9",
                (false, true),
                &["d4c-0.1.2-alpha.9-windows-x86_64.zip"],
            ),
            release(
                "0.1.2-alpha.10",
                (false, true),
                &["d4c-0.1.2-alpha.10-windows-x86_64.zip"],
            ),
            release("0.1.1", (false, false), &["d4c-0.1.1-linux-x86_64.tar.gz"]),
            release(
                "0.1.2",
                (false, false),
                &["d4c-server-0.1.2-linux-x86_64.tar.gz"],
            ),
        ];
        assert!(latest(&entries, false, "windows", "x86_64").is_none());
        assert_eq!(
            latest(&entries, true, "windows", "x86_64")
                .unwrap()
                .version
                .to_string(),
            "0.1.2-alpha.10"
        );
        assert_eq!(
            latest(&entries, false, "linux", "x86_64")
                .unwrap()
                .version
                .to_string(),
            "0.1.1"
        );
    }
}
