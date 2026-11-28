use super::{Event, HostKey, Reporter};
use crate::gateway::{
    self,
    codex::{Adapter, Output},
};
use russh::{Channel, ChannelMsg, client};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::pending,
    sync::mpsc,
    time::{Duration, SystemTime},
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::mpsc as async_mpsc,
    task::JoinSet,
    time::{self, Instant},
};
use url::Url;

pub(super) enum Command {
    Retry,
    Login(bool),
    Cancel,
    SubmitRedirect(String),
}

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LimitWindow {
    pub used_percent: i64,
    pub window_duration_mins: Option<u64>,
    pub resets_at: Option<u64>,
}

impl LimitWindow {
    pub fn remaining(&self) -> i64 {
        100 - self.used_percent.clamp(0, 100)
    }
}

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LimitBucket {
    pub limit_name: Option<String>,
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
}

impl LimitBucket {
    pub fn windows(&self) -> impl Iterator<Item = (usize, &LimitWindow)> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .enumerate()
            .filter_map(|(index, window)| Some((index, window?)))
    }
}

#[derive(Clone)]
pub(super) struct Quota {
    pub buckets: BTreeMap<String, LimitBucket>,
    pub checked: SystemTime,
}

impl Quota {
    fn parse(value: &Value, previous: Option<&Self>) -> Option<Self> {
        let mut buckets = previous.map_or_else(BTreeMap::new, |quota| quota.buckets.clone());
        let mut insert = |id: &str, value: &Value| {
            let Ok(mut bucket) = serde_json::from_value::<LimitBucket>(value.clone()) else {
                return false;
            };
            if let Some(old) = buckets.get(id) {
                if value.get("primary").is_none() {
                    bucket.primary = old.primary.clone();
                }
                if value.get("secondary").is_none() {
                    bucket.secondary = old.secondary.clone();
                }
                if value.get("limitName").is_none() {
                    bucket.limit_name = old.limit_name.clone();
                }
            }
            if bucket.windows().next().is_none() {
                buckets.remove(id);
            } else {
                buckets.insert(id.to_owned(), bucket);
            }
            true
        };
        let mut seen = false;
        if let Some(by_id) = value["rateLimitsByLimitId"]
            .as_object()
            .filter(|map| !map.is_empty())
        {
            for (id, bucket) in by_id {
                seen |= insert(id, bucket);
            }
        } else if let Some(bucket) = value.get("rateLimits") {
            seen = insert(bucket["limitId"].as_str().unwrap_or("codex"), bucket);
        }
        (seen && !buckets.is_empty()).then(|| Self {
            buckets,
            checked: SystemTime::now(),
        })
    }
}

#[derive(Default)]
pub(super) struct Status {
    pub account: Option<String>,
    pub quota_scope: Option<(String, String)>,
    pub chatgpt: bool,
    pub quota: Option<Quota>,
    pub quota_live: bool,
    pub unavailable: bool,
    pub busy: bool,
    pub can_cancel: bool,
    pub browser_callback: bool,
    pub device: Option<(String, String)>,
    pub error: Option<String>,
}

impl Status {
    pub fn label(&self) -> String {
        if let Some(account) = &self.account {
            format!("Signed in · {account}")
        } else if self.unavailable {
            "Unavailable".into()
        } else if self.busy {
            if self.device.is_some() {
                "Waiting for device approval".into()
            } else {
                if self.can_cancel {
                    "Waiting for browser sign-in".into()
                } else {
                    "Checking sign-in…".into()
                }
            }
        } else {
            "Not signed in".into()
        }
    }
}

#[derive(Default)]
struct Auth {
    account: Option<String>,
    quota_scope: Option<(String, String)>,
    chatgpt: bool,
    quota: Option<Quota>,
    quota_live: bool,
    quota_request: u64,
    next_quota_read: Option<Instant>,
    operation: Option<&'static str>,
    deadline: Option<Instant>,
    login_id: Option<String>,
    device: Option<(String, String)>,
    error: Option<String>,
    callback: Option<Callback>,
}

impl Auth {
    fn publish(&self, report: &Reporter) {
        report.send(Event::Codex(Status {
            account: self.account.clone(),
            quota_scope: self.quota_scope.clone(),
            chatgpt: self.chatgpt,
            quota: self.quota.clone(),
            quota_live: self.quota_live,
            busy: self.operation.is_some() || self.login_id.is_some(),
            can_cancel: self.login_id.is_some(),
            browser_callback: self.callback.is_some(),
            device: self.device.clone(),
            error: self.error.clone(),
            ..Status::default()
        }));
    }

    async fn request(
        &mut self,
        channel: &Channel<client::Msg>,
        id: &'static str,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        self.operation = Some(id);
        self.deadline = Some(Instant::now() + Duration::from_secs(30));
        send(channel, json!({"id":id,"method":method,"params":params})).await
    }

    async fn read_account(&mut self, channel: &Channel<client::Msg>) -> Result<(), String> {
        self.request(channel, "auth-account", "account/read", json!({}))
            .await
    }

    async fn read_quota(&mut self, channel: &Channel<client::Msg>) -> Result<(), String> {
        self.quota_request += 1;
        self.next_quota_read = Some(Instant::now() + Duration::from_secs(60));
        send(
            channel,
            json!({
                "id": format!("quota-{}", self.quota_request),
                "method": "account/rateLimits/read",
            }),
        )
        .await
    }

    fn update_quota(&mut self, quota: Option<Quota>) {
        self.quota_live = quota.is_some();
        if quota.is_some() {
            self.quota = quota;
        }
    }

    async fn cancel(&mut self, channel: &Channel<client::Msg>) -> Result<(), String> {
        self.callback = None;
        self.device = None;
        if let Some(id) = self.login_id.take() {
            self.request(
                channel,
                "auth-cancel",
                "account/login/cancel",
                json!({"loginId":id}),
            )
            .await?;
        }
        Ok(())
    }
}

fn official_url(value: &Value) -> Result<Url, String> {
    let url = value
        .as_str()
        .and_then(|value| Url::parse(value).ok())
        .filter(|url| {
            url.scheme() == "https"
                && matches!(url.host_str(), Some("auth.openai.com" | "chatgpt.com"))
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
        });
    url.ok_or_else(|| "Codex returned an unexpected sign-in URL.".into())
}

fn callback_url(url: &Url) -> Result<Url, String> {
    url.query_pairs()
        .find_map(|(key, value)| (key == "redirect_uri").then_some(value))
        .and_then(|value| Url::parse(&value).ok())
        .filter(|url| {
            url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
                && url.path() == "/auth/callback"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.port().is_some_and(|port| port != 0)
        })
        .ok_or_else(|| "Codex returned an unexpected browser callback.".into())
}

struct Callback {
    ipv4: Option<TcpListener>,
    ipv6: Option<TcpListener>,
    redirect: Url,
    state: Option<String>,
}

impl Callback {
    async fn bind(url: &Url) -> Result<Self, String> {
        let redirect = callback_url(url)?;
        let port = redirect.port().unwrap();
        let state = url.query_pairs().find_map(|(key, value)| {
            (key == "state" && !value.is_empty()).then(|| value.into_owned())
        });
        let ipv4 = TcpListener::bind(("127.0.0.1", port)).await.ok();
        let ipv6 = TcpListener::bind(("::1", port)).await.ok();
        Ok(Self {
            ipv4,
            ipv6,
            redirect,
            state,
        })
    }

    fn port(&self) -> u16 {
        self.redirect.port().unwrap()
    }

    fn request(&self, pasted: &str) -> Result<String, String> {
        let pasted = pasted.trim();
        let invalid =
            || "Paste the full localhost redirect URL from this sign-in attempt.".to_owned();
        if pasted.len() > 16_384 || pasted.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(invalid());
        }
        let mut url = Url::parse(pasted).map_err(|_| invalid())?;
        let query = url.query().ok_or_else(invalid)?.to_owned();
        let mut states = url.query_pairs().filter(|(key, _)| key == "state");
        let matches_state = self.state.as_deref().is_some_and(|expected| {
            states.next().is_some_and(|(_, value)| value == expected) && states.next().is_none()
        });
        let has_result = url
            .query_pairs()
            .any(|(key, value)| matches!(key.as_ref(), "code" | "error") && !value.is_empty());
        url.set_query(None);
        if url != self.redirect || !matches_state || !has_result {
            return Err(invalid());
        }
        Ok(format!(
            "GET {}?{query} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            url.path(),
            url.host_str().unwrap(),
            self.port()
        ))
    }

    async fn submit(
        &self,
        session: &client::Handle<HostKey>,
        pasted: &str,
        forwards: &mut JoinSet<()>,
    ) -> Result<(), String> {
        let request = self.request(pasted)?;
        if forwards.len() >= 8 {
            return Err("Sign-in is already connecting. Try again in a moment.".into());
        }
        let mut remote = time::timeout(Duration::from_secs(3), async {
            let channel = session
                .channel_open_direct_tcpip("127.0.0.1", u32::from(self.port()), "127.0.0.1", 0)
                .await
                .ok()?;
            let mut remote = channel.into_stream();
            remote.write_all(request.as_bytes()).await.ok()?;
            Some(remote)
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            "SSH couldn't forward the redirect. Try again or use a device code.".to_owned()
        })?;
        forwards.spawn(async move {
            let _ = time::timeout(
                Duration::from_secs(30),
                tokio::io::copy(&mut remote, &mut tokio::io::sink()),
            )
            .await;
        });
        Ok(())
    }

    async fn accept(callback: Option<&Self>) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
        let Some(callback) = callback else {
            return pending().await;
        };
        async fn accept(
            listener: Option<&TcpListener>,
        ) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
            match listener {
                Some(listener) => listener.accept().await,
                None => pending().await,
            }
        }
        tokio::select! {
            result = accept(callback.ipv4.as_ref()) => result,
            result = accept(callback.ipv6.as_ref()) => result,
        }
    }
}

async fn send(channel: &Channel<client::Msg>, value: Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    channel.data_bytes(line).await.map_err(|e| e.to_string())
}

pub(super) async fn serve(
    session: &client::Handle<HostKey>,
    commands: &mut async_mpsc::UnboundedReceiver<Command>,
    report: &Reporter,
) -> Result<(), String> {
    loop {
        report.send(Event::Codex(Status {
            busy: true,
            ..Status::default()
        }));
        if let Err(error) = run(session, commands, report).await {
            report.send(Event::Codex(Status {
                unavailable: true,
                error: Some(error),
                ..Status::default()
            }));
        }
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Retry) => break,
                    None => return Ok(()),
                    _ => {}
                },
                _ = time::sleep(Duration::from_secs(1)) => {
                    if session.is_closed() { return Err("SSH connection lost.".into()); }
                }
            }
        }
    }
}

async fn run(
    session: &client::Handle<HostKey>,
    commands: &mut async_mpsc::UnboundedReceiver<Command>,
    report: &Reporter,
) -> Result<(), String> {
    let mut channel = time::timeout(Duration::from_secs(10), session.channel_open_session())
        .await
        .map_err(|_| "Opening the Codex SSH channel timed out.".to_owned())?
        .map_err(|_| "Couldn't open the Codex SSH channel.".to_owned())?;
    let (out, requests) = mpsc::channel();
    let (events, incoming) = mpsc::channel();
    report.send(Event::CodexLink(gateway::Gateway::from_channels(
        out, incoming,
    )));
    let emit = |event| {
        let _ = events.send(event);
        report.ctx.request_repaint();
    };
    let result = exchange(session, &mut channel, commands, report, requests, &emit).await;
    let _ = time::timeout(Duration::from_secs(1), channel.close()).await;
    if let Err(error) = &result {
        emit(gateway::Event::Error(error.clone()));
    }
    result
}

async fn exchange(
    session: &client::Handle<HostKey>,
    channel: &mut Channel<client::Msg>,
    commands: &mut async_mpsc::UnboundedReceiver<Command>,
    report: &Reporter,
    requests: mpsc::Receiver<Value>,
    emit: &impl Fn(gateway::Event),
) -> Result<(), String> {
    channel
        .exec(true, "codex app-server --listen stdio://")
        .await
        .map_err(|e| e.to_string())?;
    send(channel, gateway::initialize()).await?;
    let mut initialized = false;
    let mut auth = Auth {
        deadline: Some(Instant::now() + Duration::from_secs(30)),
        ..Auth::default()
    };
    let mut lines = Vec::new();
    let mut adapter = Adapter::default();
    let mut forwards = JoinSet::new();
    let mut tick = time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Login(device)) if initialized && auth.operation.is_none() && auth.login_id.is_none() => {
                    auth.error = None;
                    let kind = if device { "chatgptDeviceCode" } else { "chatgpt" };
                    let id = if device { "auth-device" } else { "auth-browser" };
                    auth.request(channel, id, "account/login/start", json!({"type":kind})).await?;
                    auth.publish(report);
                }
                Some(Command::Cancel) if auth.login_id.is_some() => {
                    forwards.abort_all();
                    auth.cancel(channel).await?;
                    auth.publish(report);
                }
                Some(Command::SubmitRedirect(url)) => {
                    if let Some(callback) = &auth.callback {
                        auth.error = callback.submit(session, &url, &mut forwards).await.err();
                        auth.publish(report);
                    }
                }
                None => return Ok(()),
                _ => {}
            },
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) => {
                    lines.extend_from_slice(&data);
                    let mut consumed = 0;
                    for line in lines.split_inclusive(|byte| *byte == b'\n') {
                        if !line.ends_with(b"\n") { break; }
                        consumed += line.len();
                        if line.iter().all(u8::is_ascii_whitespace) { continue; }
                        let value: Value = serde_json::from_slice(line).map_err(|_| "Invalid Codex app-server response.".to_owned())?;
                        if !initialized && value["id"] == "initialize" {
                            if !value["result"].is_object() {
                                return Err("Codex app-server initialization failed.".into());
                            }
                            initialized = true;
                            send(channel, json!({"method":"initialized"})).await?;
                            auth.read_account(channel).await?;
                        } else if !handle_auth(&value, &mut auth, channel, report, emit).await? {
                            adapter.incoming(value);
                        }
                    }
                    lines.drain(..consumed);
                    if lines.len() > 64 * 1024 * 1024 {
                        return Err("Codex sent an oversized response.".into());
                    }
                }
                Some(ChannelMsg::Failure | ChannelMsg::ExitStatus { .. } | ChannelMsg::Eof | ChannelMsg::Close) | None => {
                    return Err(if !initialized {
                        "Couldn't start Codex. Install a recent Codex CLI on this server and make it available on the SSH user's PATH.".into()
                    } else {
                        "The Codex app-server session ended.".into()
                    });
                }
                _ => {}
            },
            result = Callback::accept(auth.callback.as_ref()) => {
                let port = auth.callback.as_ref().unwrap().port();
                let remote = match result {
                    Ok((local, peer)) if forwards.len() < 8 => {
                        match time::timeout(Duration::from_secs(3), session.channel_open_direct_tcpip(
                            "127.0.0.1", u32::from(port), peer.ip().to_string(), u32::from(peer.port()),
                        )).await {
                            Ok(Ok(remote)) => {
                                forwards.spawn(async move {
                                    let mut local = local;
                                    let mut remote = remote.into_stream();
                                    let _ = time::timeout(Duration::from_secs(30), tokio::io::copy_bidirectional(&mut local, &mut remote)).await;
                                });
                                Ok(())
                            }
                            _ => Err(()),
                        }
                    }
                    Ok(_) => Ok(()),
                    Err(_) => Err(()),
                };
                if remote.is_err() {
                    auth.error = Some("SSH couldn't forward the sign-in callback. Use a device code instead.".into());
                    auth.cancel(channel).await?;
                    auth.publish(report);
                }
            },
            _ = forwards.join_next(), if !forwards.is_empty() => {},
            _ = tick.tick() => {
                if let Some(deadline) = auth.deadline && Instant::now() >= deadline {
                    if auth.operation.is_some() || !initialized {
                        return Err("The Codex app-server request timed out. Retry Codex on this server.".into());
                    }
                    auth.error = Some("Codex sign-in timed out.".into());
                    auth.cancel(channel).await?;
                    auth.publish(report);
                }
                if auth.operation.is_none() && auth.login_id.is_none()
                    && auth.next_quota_read.is_some_and(|next| Instant::now() >= next)
                {
                    if auth.quota_live && auth.quota.as_ref().is_some_and(|quota|
                        quota.checked.elapsed().unwrap_or_default() > Duration::from_secs(90)) {
                        auth.quota_live = false;
                        auth.publish(report);
                    }
                    auth.read_quota(channel).await?;
                }
                if auth.account.is_some() {
                    for request in requests.try_iter().take(32) {
                        adapter.command(request);
                    }
                }
            },
        }
        for output in adapter.output.drain(..) {
            match output {
                Output::Wire(value) => send(channel, value).await?,
                Output::Ui(value) => emit(gateway::Event::Data(value)),
                Output::Organizer(call) => emit(gateway::Event::Organizer(call)),
            }
        }
    }
}

async fn handle_auth(
    value: &Value,
    auth: &mut Auth,
    channel: &Channel<client::Msg>,
    report: &Reporter,
    emit: &impl Fn(gateway::Event),
) -> Result<bool, String> {
    let id = value["id"].as_str().unwrap_or("");
    let params = &value["params"];
    let result = &value["result"];
    if auth.operation == Some(id) {
        auth.operation = None;
        auth.deadline = None;
        match id {
            "auth-account" => {
                if !result.is_object() {
                    return Err("Couldn't read the Codex sign-in status. Update Codex on the server and retry.".into());
                }
                let previous = auth.account.take();
                let account = &result["account"];
                auth.account = account.is_object().then(|| {
                    account["email"]
                        .as_str()
                        .unwrap_or(match account["type"].as_str() {
                            Some("apiKey") => "API key",
                            _ => "ChatGPT",
                        })
                        .to_owned()
                });
                auth.chatgpt = account["type"] == "chatgpt";
                // The same email can belong to several organization workspaces.
                // Only group an explicitly identified workspace or a personal plan.
                let scope = account["workspaceId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .or_else(|| match account["planType"].as_str() {
                        Some("free" | "go" | "plus" | "pro") => Some("personal"),
                        _ => None,
                    });
                auth.quota_scope = auth
                    .chatgpt
                    .then(|| {
                        Some((
                            account["email"].as_str()?.to_ascii_lowercase(),
                            scope?.to_owned(),
                        ))
                    })
                    .flatten();
                auth.quota = None;
                auth.quota_live = false;
                auth.next_quota_read = None;
                auth.quota_request += 1;
                if auth.chatgpt {
                    auth.read_quota(channel).await?;
                }
                match (&previous, &auth.account) {
                    (None, Some(_)) => emit(gateway::Event::Status("Connected".into())),
                    (Some(_), None) => emit(gateway::Event::Error(
                        "Codex signed out on the SSH server.".into(),
                    )),
                    _ => {}
                }
            }
            "auth-cancel" => auth.read_account(channel).await?,
            "auth-browser" | "auth-device" => {
                if let Some(login_id) = result["loginId"].as_str() {
                    auth.login_id = Some(login_id.into());
                    auth.deadline = Some(Instant::now() + Duration::from_secs(15 * 60));
                    let setup = if id == "auth-browser" {
                        async {
                            let url = official_url(&result["authUrl"])?;
                            auth.callback = Some(Callback::bind(&url).await?);
                            report.send(Event::OpenCodexUrl(url.into()));
                            Ok::<_, String>(())
                        }
                        .await
                    } else {
                        official_url(&result["verificationUrl"]).and_then(|url| {
                            let code = result["userCode"]
                                .as_str()
                                .filter(|code| {
                                    !code.is_empty()
                                        && code.len() <= 64
                                        && code.bytes().all(|byte| {
                                            byte.is_ascii_alphanumeric() || byte == b'-'
                                        })
                                })
                                .ok_or_else(|| {
                                    "Codex did not return a valid device code.".to_owned()
                                })?;
                            auth.device = Some((url.into(), code.into()));
                            Ok(())
                        })
                    };
                    if let Err(error) = setup {
                        auth.error = Some(error);
                        auth.cancel(channel).await?;
                    }
                } else {
                    auth.error = Some(if id == "auth-device" {
                        "Couldn't start device sign-in. Check the server's network and enable device code authorization in ChatGPT security settings."
                    } else {
                        "Couldn't start browser sign-in. Try a device code or check the server's Codex version."
                    }.into());
                }
            }
            _ => {}
        }
    } else if let Some(request) = id.strip_prefix("quota-") {
        if auth.chatgpt
            && auth.operation.is_none()
            && auth.login_id.is_none()
            && request.parse::<u64>().ok() == Some(auth.quota_request)
        {
            auth.update_quota(Quota::parse(result, None));
        }
    } else if value["method"] == "account/rateLimits/updated" {
        if auth.chatgpt
            && auth.operation.is_none()
            && auth.login_id.is_none()
            && let Some(quota) = Quota::parse(params, auth.quota.as_ref())
        {
            auth.update_quota(Some(quota));
            // Don't replace this push with an older in-flight snapshot.
            auth.quota_request += 1;
            auth.next_quota_read = Some(Instant::now() + Duration::from_secs(60));
        }
    } else if value["method"] == "account/login/completed" {
        if auth
            .login_id
            .as_deref()
            .is_some_and(|id| params["loginId"] == id)
        {
            auth.login_id = None;
            auth.device = None;
            auth.callback = None;
            if params["success"] != true {
                auth.error = Some("Codex sign-in didn't complete. Try signing in again.".into());
            }
            auth.read_account(channel).await?;
        }
    } else if value["method"] == "account/updated" {
        if auth.operation.is_none() && auth.login_id.is_none() {
            auth.read_account(channel).await?;
        }
    } else {
        return Ok(false);
    }
    auth.publish(report);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_callback_cannot_forward_to_an_arbitrary_host_or_path() {
        for (redirect, expected) in [
            ("http://localhost:1455/auth/callback", Ok(1455)),
            ("http://127.0.0.1:1455/auth/callback", Ok(1455)),
            ("http://localhost:1455/other", Err(())),
            ("http://example.com:1455/auth/callback", Err(())),
        ] {
            let mut url = Url::parse("https://auth.openai.com/oauth/authorize").unwrap();
            url.query_pairs_mut().append_pair("redirect_uri", redirect);
            assert_eq!(
                callback_url(&url)
                    .map(|url| url.port().unwrap())
                    .map_err(|_| ()),
                expected
            );
        }
        assert!(official_url(&json!("https://auth.openai.com.example.org/")).is_err());
        assert!(official_url(&json!("https://login@auth.openai.com/")).is_err());
        assert!(official_url(&json!("https://auth.openai.com/codex/device")).is_ok());
    }

    #[test]
    fn pasted_callback_must_match_active_redirect_and_state() {
        let callback = Callback {
            ipv4: None,
            ipv6: None,
            redirect: Url::parse("http://localhost:1455/auth/callback").unwrap(),
            state: Some("active".into()),
        };
        assert!(
            callback
                .request("http://localhost:1455/auth/callback?code=ok&state=active")
                .is_ok()
        );
        for url in [
            "https://example.com:1455/auth/callback?code=ok&state=active",
            "http://localhost:1456/auth/callback?code=ok&state=active",
            "http://localhost:1455/other?code=ok&state=active",
            "http://localhost:1455/auth/callback?code=ok&state=expired",
            "http://localhost:1455/auth/callback?code=ok&state=active&state=expired",
            "http://localhost:1455/auth/callback?state=active",
        ] {
            assert!(callback.request(url).is_err());
        }
    }
}
