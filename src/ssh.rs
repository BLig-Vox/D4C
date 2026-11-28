mod codex;

use crate::{gateway, preferences};
use eframe::egui;
use russh::{
    client,
    keys::{
        HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key,
        ssh_key,
    },
};
use shadcn_egui::components::{
    button::{Button, ButtonVariant},
    input::Input,
};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::sync::oneshot;

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct Endpoint {
    host: String,
    port: u16,
    user: String,
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let host = &self.host;
        if host.contains(':') {
            write!(f, "{}@[{host}]:{}", self.user, self.port)
        } else {
            write!(f, "{}@{host}:{}", self.user, self.port)
        }
    }
}

impl Endpoint {
    fn chat_id(&self) -> String {
        format!("ssh://{self}")
    }
}

pub(crate) fn is_chat_id(id: &str) -> bool {
    id.starts_with("ssh://")
}

enum Credential {
    Password(String),
    Key { path: PathBuf, passphrase: String },
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Authentication {
    #[default]
    Password,
    KeyFile,
    D4cKey,
}

impl Authentication {
    fn label(self) -> &'static str {
        match self {
            Self::Password => "Password",
            Self::KeyFile => "Private key file",
            Self::D4cKey => "D4C key",
        }
    }
}

fn managed_key_path() -> io::Result<PathBuf> {
    Ok(crate::platform::config_directory()?
        .join("ssh")
        .join("id_ed25519"))
}

fn managed_public_key(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if !path.try_exists()? {
        let directory = path.parent().expect("managed key has a parent directory");
        std::fs::create_dir_all(directory)?;
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let key = PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(
            signing_key.as_bytes(),
        ));
        // NamedTempFile creates a private file on Unix; never overwrite an existing identity.
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(key.to_openssh(ssh_key::LineEnding::LF)?.as_bytes())?;
        temporary.as_file().sync_all()?;
        if let Err(error) = temporary.persist_noclobber(path) {
            if error.error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error.error.into());
            }
        }
    }
    Ok(load_secret_key(path, None)?.public_key().to_openssh()?)
}

enum Event {
    Verify {
        fingerprint: String,
        reply: oneshot::Sender<bool>,
    },
    Connected,
    Disconnected(String),
    Codex(codex::Status),
    CodexLink(gateway::Gateway),
    OpenCodexUrl(String),
}

struct Reporter {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl Reporter {
    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

struct HostKey {
    report: Reporter,
    accepted: Option<String>,
}

impl client::Handler for HostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        if let Some(accepted) = &self.accepted {
            return Ok(accepted == &fingerprint);
        }
        let (reply, response) = oneshot::channel();
        self.report.send(Event::Verify {
            fingerprint: fingerprint.clone(),
            reply,
        });
        let trust = response.await.unwrap_or(false);
        if trust {
            self.accepted = Some(fingerprint);
        }
        Ok(trust)
    }
}

async fn connect(
    endpoint: Endpoint,
    credential: Credential,
    report: Reporter,
    fingerprint: Option<String>,
) -> Result<client::Handle<HostKey>, String> {
    let config = client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 2,
        ..Default::default()
    };
    let pinned = fingerprint.is_some();
    let handler = HostKey {
        report,
        accepted: fingerprint,
    };
    let mut session = tokio::time::timeout(
        Duration::from_secs(120),
        client::connect(
            Arc::new(config),
            (endpoint.host.as_str(), endpoint.port),
            handler,
        ),
    )
    .await
    .map_err(|_| "SSH connection or fingerprint confirmation timed out.".to_owned())?
    .map_err(|error| match error {
        russh::Error::UnknownKey if pinned => {
            "Saved SSH host key does not match. Verify the server before trusting a new key.".into()
        }
        russh::Error::UnknownKey => "Host key not accepted.".to_owned(),
        _ => format!("Couldn't connect over SSH: {error}"),
    })?;
    let authentication = async {
        match credential {
            Credential::Password(password) => session
                .authenticate_password(endpoint.user, password)
                .await
                .map_err(|error| format!("SSH authentication failed: {error}")),
            Credential::Key { path, passphrase } => {
                let key = tokio::task::spawn_blocking(move || {
                    let passphrase = (!passphrase.is_empty()).then_some(passphrase.as_str());
                    load_secret_key(path, passphrase)
                })
                .await
                .map_err(|error| format!("Couldn't load the private key: {error}"))?
                .map_err(|error| format!("Couldn't read or unlock the private key: {error}"))?;
                let hash = session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|error| format!("Couldn't negotiate SSH key authentication: {error}"))?
                    .flatten();
                session
                    .authenticate_publickey(
                        endpoint.user,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                    .map_err(|error| format!("SSH key authentication failed: {error}"))
            }
        }
    };
    if !tokio::time::timeout(Duration::from_secs(20), authentication)
        .await
        .map_err(|_| "SSH authentication timed out.".to_owned())??
        .success()
    {
        return Err("SSH authentication was rejected.".to_owned());
    }
    Ok(session)
}

struct Connection {
    endpoint: Endpoint,
    events: mpsc::Receiver<Event>,
    // Dropping this cancels the worker, including in-progress handshakes.
    _stop: oneshot::Sender<()>,
    status: String,
    codex: codex::Status,
    redirect_input: String,
    chat: Option<gateway::Gateway>,
    commands: tokio::sync::mpsc::UnboundedSender<codex::Command>,
}

struct Pending {
    connection: Connection,
    verify: Option<(String, oneshot::Sender<bool>)>,
    remember: bool,
    fingerprint: Option<String>,
}

const SAVED_SERVERS_FILE: &str = "ssh-servers.json";
const CONNECTING: &str = "Connecting…";

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct SavedServer {
    endpoint: Endpoint,
    fingerprint: String,
}

#[derive(Default)]
pub struct Servers {
    host: String,
    port: String,
    user: String,
    authentication: Authentication,
    key_path: String,
    public_key: String,
    key_copied: bool,
    secret: String,
    pending: Option<Pending>,
    servers: Vec<Connection>,
    saved: Vec<SavedServer>,
    restore_pending: bool,
    error: String,
}

fn ssh_field(ui: &mut egui::Ui, label: &str, input: Input<'_>) {
    let label = ui.label(label);
    let field = ui.add(input).labelled_by(label.id);
    if cfg!(target_os = "android") && field.has_focus() {
        // The keyboard can shrink the viewport a frame after the field receives focus.
        let bottom = ui.clip_rect().bottom();
        let id = field.id.with("keyboard-scroll");
        let ime = ui.ctx().output(|output| output.ime);
        let gained_focus = field.gained_focus();
        let (scrolling, keep_ime) = ui.data_mut(|data| {
            let (previous_bottom, previous_ime, scrolling) = data
                .get_temp::<(f32, Option<egui::output::IMEOutput>, bool)>(id)
                .unwrap_or((bottom, None, false));
            let scrolling = field.rect.bottom() > bottom
                && (scrolling || gained_focus || bottom < previous_bottom);
            data.insert_temp(id, (bottom, ime.or(previous_ime), scrolling));
            (scrolling, previous_ime.filter(|_| scrolling))
        });
        if let Some(ime) = keep_ime {
            // egui omits IME output while the field is clipped. Keep the keyboard
            // open until the scroll brings the field into view.
            ui.output_mut(|output| {
                output.ime.get_or_insert(ime);
            });
        }
        if scrolling {
            ui.scroll_with_delta_animation(
                egui::vec2(
                    0.0,
                    bottom - field.rect.bottom() - ui.spacing().item_spacing.y,
                ),
                egui::style::ScrollAnimation::none(),
            );
        }
    }
}

impl Servers {
    pub fn load() -> Self {
        let saved: Vec<SavedServer> =
            serde_json::from_value(preferences::read(SAVED_SERVERS_FILE)).unwrap_or_default();
        Self {
            restore_pending: !saved.is_empty(),
            saved,
            ..Self::default()
        }
    }

    pub fn remembers(&self, url: &str) -> bool {
        self.saved.iter().any(|entry| entry.endpoint.chat_id() == url)
    }

    fn save(&mut self, saved: Vec<SavedServer>) -> io::Result<()> {
        preferences::write(SAVED_SERVERS_FILE, &serde_json::to_value(&saved)?)?;
        self.saved = saved;
        Ok(())
    }

    fn remember(&mut self, endpoint: Endpoint, fingerprint: String) -> io::Result<()> {
        let mut saved = self.saved.clone();
        saved.retain(|entry| entry.endpoint != endpoint);
        saved.push(SavedServer { endpoint, fingerprint });
        self.save(saved)
    }

    fn forget(&mut self, endpoint: &Endpoint) -> io::Result<()> {
        let saved: Vec<_> = self.saved.iter().filter(|entry| &entry.endpoint != endpoint).cloned().collect();
        if saved.len() == self.saved.len() {
            return Ok(());
        }
        self.save(saved)
    }

    pub fn retry_saved(&mut self, url: &str, ctx: &egui::Context) -> bool {
        if self.servers.iter().any(|server| {
            server.endpoint.chat_id() == url
                && (server.status == CONNECTING
                    || (server.status == "Connected"
                        && !server.codex.unavailable
                        && server.codex.account.is_some()))
        }) {
            return true;
        }
        if self.is_connected(url) {
            return false;
        }
        let Some(saved) = self.saved.iter().find(|entry| entry.endpoint.chat_id() == url).cloned() else {
            return false;
        };
        let result = managed_key_path()
            .map_err(|error| error.to_string())
            .and_then(|path| {
                Connection::start(
                    saved.endpoint.clone(),
                    Credential::Key { path, passphrase: String::new() },
                    Some(saved.fingerprint),
                    ctx,
                )
            });
        match result {
            Ok(server) => {
                if let Some(existing) = self.servers.iter_mut().find(|row| row.endpoint == server.endpoint) {
                    *existing = server;
                } else {
                    self.servers.push(server);
                }
                true
            }
            Err(error) => {
                self.error = error;
                false
            }
        }
    }

    pub fn status(&self, url: &str) -> Option<&str> {
        self.servers.iter().find(|row| row.endpoint.chat_id() == url).map(|row| match row.status.as_str() {
            "Connected" if row.codex.unavailable => "Codex is unavailable. Check Connections in Settings.",
            "Connected" if row.codex.account.is_none() && !row.codex.busy => "Sign in to Codex in Connections.",
            "Connected" => CONNECTING,
            other => other,
        })
    }

    pub fn chat_connections(&self) -> impl Iterator<Item = preferences::Connection> + '_ {
        self.servers.iter().filter_map(|server| {
            (server.status == "Connected"
                && !server.codex.unavailable
                && server.codex.account.is_some())
            .then(|| preferences::Connection {
                name: server.endpoint.to_string(),
                url: server.endpoint.chat_id(),
            })
        })
    }

    pub fn is_connected(&self, url: &str) -> bool {
        self.servers
            .iter()
            .any(|server| server.status == "Connected" && server.endpoint.chat_id() == url)
    }

    pub fn take_chat(&mut self, url: &str) -> Option<gateway::Gateway> {
        self.servers
            .iter_mut()
            .find(|server| {
                server.endpoint.chat_id() == url
                    && !server.codex.unavailable
                    && server.codex.account.is_some()
            })?
            .chat
            .take()
    }

    fn codex_quota_server(&self, url: &str) -> Option<&Connection> {
        self.servers
            .iter()
            .find(|server| server.endpoint.chat_id() == url)
    }

    pub fn codex_usage_ui(&self, ui: &mut egui::Ui, url: &str) {
        ui.small("Codex usage");
        let server = self.codex_quota_server(url);
        if let Some(server) = server.filter(|server| server.codex.chatgpt) {
            server.quota_ui(ui, self.shared_count(server), true);
        } else {
            let reason = if server.is_some_and(|server| server.codex.account.is_some()) {
                "Usage is unavailable for API key sign-in."
            } else {
                "Waiting for Codex usage from this server."
            };
            ui.small("—").on_hover_text(reason);
        }
    }

    fn shared_count(&self, server: &Connection) -> usize {
        server.codex.quota_scope.as_ref().map_or(0, |scope| {
            self.servers
                .iter()
                .filter(|other| other.codex.quota_scope.as_ref() == Some(scope))
                .count()
        })
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.restore_pending) {
            let urls: Vec<_> = self.saved.iter().map(|entry| entry.endpoint.chat_id()).collect();
            for url in urls {
                self.retry_saved(&url, ctx);
            }
        }
        for server in &mut self.servers {
            loop {
                match server.events.try_recv() {
                    Ok(Event::Codex(mut status)) => {
                        if !status.browser_callback {
                            server.redirect_input.clear();
                        }
                        if status.unavailable && server.codex.quota.is_some() {
                            status.account = server.codex.account.take();
                            status.quota_scope = server.codex.quota_scope.take();
                            status.chatgpt = server.codex.chatgpt;
                            status.quota = server.codex.quota.take();
                        }
                        server.codex = status;
                    }
                    Ok(Event::CodexLink(link)) => server.chat = Some(link),
                    Ok(Event::OpenCodexUrl(url)) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                    Ok(Event::Connected) => server.status = "Connected".into(),
                    Ok(Event::Disconnected(message)) => server.status = message,
                    Err(mpsc::TryRecvError::Disconnected)
                        if server.status == "Connected" || server.status == CONNECTING =>
                    {
                        server.status = "Disconnected".into();
                        break;
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
            if server.status != "Connected" {
                server.chat = None;
                server.redirect_input.clear();
            }
        }
        let Some(pending) = &mut self.pending else {
            return;
        };
        match pending.connection.events.try_recv() {
            Ok(Event::Verify { fingerprint, reply }) => {
                pending.verify = Some((fingerprint, reply));
            }
            Ok(Event::Connected) => {
                let Pending { connection: mut server, remember, fingerprint, .. } = self.pending.take().unwrap();
                if remember
                    && let Some(fingerprint) = fingerprint
                    && let Err(error) = self.remember(server.endpoint.clone(), fingerprint)
                {
                    self.error = format!("Couldn't save the SSH server: {error}");
                }
                server.status = "Connected".into();
                self.servers.retain(|row| row.endpoint != server.endpoint);
                self.servers.push(server);
                ctx.request_repaint();
            }
            Ok(Event::Disconnected(message)) => {
                self.error = message;
                self.pending = None;
            }
            Ok(_) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = "SSH connection ended.".into();
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui) -> Option<String> {
        if let Some(pending) = &mut self.pending {
            ui.label(pending.connection.endpoint.to_string());
            if let Some((fingerprint, _)) = &pending.verify {
                ui.label("Compare this fingerprint with one from your server administrator.");
                ui.add(egui::Label::new(egui::RichText::new(fingerprint).monospace()).wrap());
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&pending.connection.status);
                });
            }
            let cancel = ui
                .horizontal_wrapped(|ui| {
                    if pending.verify.is_some() && ui.button("Fingerprint matches").clicked() {
                        let (fingerprint, reply) = pending.verify.take().unwrap();
                        pending.fingerprint = Some(fingerprint);
                        let _ = reply.send(true);
                        pending.connection.status = "Authenticating…".into();
                    }
                    ui.button("Cancel").clicked()
                })
                .inner;
            if cancel {
                self.pending = None;
            }
            return None;
        }
        let (_, open) = self.render_rows(ui, "", true);
        if self.servers.is_empty() {
            self.connection_form(ui);
        } else {
            ui.separator();
            egui::CollapsingHeader::new("Add SSH server")
                .default_open(!self.error.is_empty())
                .show(ui, |ui| self.connection_form(ui));
        }
        open
    }

    fn connection_form(&mut self, ui: &mut egui::Ui) {
        if ui.available_width() >= 480.0 {
            ui.columns(2, |columns| {
                self.endpoint_fields(&mut columns[0]);
                self.authentication_fields(&mut columns[1]);
            });
        } else {
            self.endpoint_fields(ui);
            self.authentication_fields(ui);
        }

        let managed_key = self.authentication == Authentication::D4cKey;
        let key_ready = managed_key && !self.public_key.is_empty();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!managed_key || key_ready, Button::new("Connect over SSH"))
                .clicked()
                && let Err(error) = self.start(ui.ctx())
            {
                self.error = error;
            }
            if key_ready {
                let label = if self.key_copied {
                    "Copied public key"
                } else {
                    "Copy public key"
                };
                if ui
                    .add(Button::new(label).variant(ButtonVariant::Outline))
                    .clicked()
                {
                    ui.copy_text(self.public_key.clone());
                    self.key_copied = true;
                }
            } else if ui
                .add(Button::new("Set up D4C SSH key").variant(ButtonVariant::Outline))
                .clicked()
            {
                self.error.clear();
                match managed_key_path()
                    .map_err(Into::into)
                    .and_then(|path| managed_public_key(&path))
                {
                    Ok(public_key) => {
                        self.authentication = Authentication::D4cKey;
                        self.secret.clear();
                        self.public_key = public_key;
                        self.key_copied = false;
                    }
                    Err(error) => self.error = format!("Couldn't set up the D4C SSH key: {error}"),
                }
            }
        });
        if key_ready {
            ui.weak("Add the key to your server account’s authorized_keys before connecting.");
            egui::CollapsingHeader::new("Public key and setup details").show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&self.public_key).monospace()).wrap());
                ui.weak("Linux: ~/.ssh/authorized_keys. Windows OpenSSH: see AuthorizedKeysFile in the server configuration.");
            });
        }
        if !self.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
        }
    }

    fn endpoint_fields(&mut self, ui: &mut egui::Ui) {
        let port_width = 76.0;
        let host_width = ui.available_width() - port_width - ui.spacing().item_spacing.x;
        if host_width >= 100.0 {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ssh_field(
                        ui,
                        "Host",
                        Input::new(&mut self.host)
                            .hint("host.example")
                            .max_width(host_width),
                    );
                });
                ui.vertical(|ui| {
                    ssh_field(
                        ui,
                        "Port",
                        Input::new(&mut self.port).hint("22").max_width(port_width),
                    );
                });
            });
        } else {
            ssh_field(ui, "Host", Input::new(&mut self.host).hint("host.example"));
            ssh_field(
                ui,
                "Port",
                Input::new(&mut self.port).hint("22").max_width(port_width),
            );
        }
        ssh_field(
            ui,
            "Username",
            Input::new(&mut self.user).max_width(f32::INFINITY),
        );
    }

    fn authentication_fields(&mut self, ui: &mut egui::Ui) {
        let label = ui.label("Authentication");
        let previous = self.authentication;
        egui::ComboBox::from_id_salt("ssh-auth")
            .selected_text(self.authentication.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for mode in [
                    Authentication::Password,
                    Authentication::KeyFile,
                    Authentication::D4cKey,
                ] {
                    ui.selectable_value(&mut self.authentication, mode, mode.label());
                }
            })
            .response
            .labelled_by(label.id);
        if previous != self.authentication {
            self.secret.clear();
            self.error.clear();
            self.key_copied = false;
        }

        if self.authentication == Authentication::KeyFile {
            ssh_field(
                ui,
                "Key file",
                Input::new(&mut self.key_path)
                    .hint("Private key path")
                    .max_width(f32::INFINITY),
            );
        }
        if self.authentication != Authentication::D4cKey {
            let label = if self.authentication == Authentication::KeyFile {
                "Key passphrase"
            } else {
                "Password"
            };
            ssh_field(
                ui,
                label,
                Input::new(&mut self.secret)
                    .password(true)
                    .max_width(f32::INFINITY),
            );
        }
    }

    pub fn rows_ui(&mut self, ui: &mut egui::Ui, query: &str) -> usize {
        self.render_rows(ui, query, false).0
    }

    fn render_rows(
        &mut self,
        ui: &mut egui::Ui,
        query: &str,
        settings: bool,
    ) -> (usize, Option<String>) {
        let mut count = 0;
        let mut remove = None;
        let mut retry = None;
        let mut open = None;
        let shared_counts: Vec<_> = if settings {
            self.servers
                .iter()
                .map(|server| self.shared_count(server))
                .collect()
        } else {
            Vec::new()
        };
        for (index, server) in self.servers.iter_mut().enumerate() {
            let label = server.endpoint.to_string();
            let connected = server.status == "Connected";
            let connecting = server.status == CONNECTING;
            let saved = self.saved.iter().any(|entry| entry.endpoint == server.endpoint);
            if !label.to_lowercase().contains(query)
                || (!settings
                    && connected
                    && !server.codex.unavailable
                    && server.codex.account.is_some())
            {
                continue;
            }
            count += 1;
            ui.push_id(server.endpoint.chat_id(), |ui| {
                if !settings {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 28.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if connected {
                                ui.colored_label(
                                    egui::Color32::from_rgb(120, 200, 155),
                                    "Connected",
                                );
                            } else {
                                ui.weak(if connecting { CONNECTING } else { "Offline" }).on_hover_text(&server.status);
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(label).strong())
                                            .truncate(),
                                    );
                                },
                            );
                        },
                    );
                    return;
                }
                if index > 0 {
                    ui.separator();
                }
                ui.horizontal_wrapped(|ui| {
                    ui.strong(label);
                    if connected {
                        ui.colored_label(egui::Color32::from_rgb(120, 200, 155), "Connected");
                        if server.codex_ui(ui) {
                            open = Some(server.endpoint.chat_id());
                        }
                    } else {
                        ui.weak(&server.status);
                        if saved && !connecting && ui.small_button("Reconnect").clicked() {
                            retry = Some(server.endpoint.chat_id());
                        }
                    }
                    if ui
                        .small_button(if saved { "Forget" } else if connected { "Disconnect" } else { "Remove" })
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
                if server.codex.chatgpt {
                    ui.horizontal_wrapped(|ui| server.quota_ui(ui, shared_counts[index], false));
                }
                if connected {
                    server.codex_details(ui);
                }
            });
        }
        if let Some(index) = remove {
            let endpoint = self.servers[index].endpoint.clone();
            match self.forget(&endpoint) {
                Ok(()) => { self.servers.remove(index); }
                Err(error) => self.error = format!("Couldn't forget the SSH server: {error}"),
            }
        }
        if let Some(url) = retry {
            self.retry_saved(&url, ui.ctx());
        }
        (count, open)
    }

    fn start(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let host = self.host.trim();
        let host = host
            .strip_prefix('[')
            .and_then(|ip| ip.strip_suffix(']'))
            .unwrap_or(host);
        let user = self.user.trim();
        if host.is_empty()
            || host.contains(char::is_whitespace)
            || host.contains(['/', '@', '[', ']'])
            || (host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err())
            || user.is_empty()
            || user.contains(char::is_control)
        {
            return Err("Enter an SSH hostname or IP and a username.".into());
        }
        let port = match self.port.trim() {
            "" => 22,
            value => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| "Enter a port between 1 and 65535.".to_owned())?,
        };
        let endpoint = Endpoint {
            host: host.to_owned(),
            port,
            user: user.into(),
        };
        if self
            .servers
            .iter()
            .any(|row| row.endpoint == endpoint && (row.status == "Connected" || row.status == CONNECTING))
        {
            return Err("Already connected to this SSH server.".into());
        }
        let credential = match self.authentication {
            Authentication::KeyFile => {
                if self.key_path.trim().is_empty() {
                    return Err("Enter the private key file path.".into());
                }
                Credential::Key {
                    path: self.key_path.trim().into(),
                    passphrase: std::mem::take(&mut self.secret),
                }
            }
            Authentication::D4cKey => Credential::Key {
                path: managed_key_path().map_err(|error| error.to_string())?,
                passphrase: String::new(),
            },
            Authentication::Password => Credential::Password(std::mem::take(&mut self.secret)),
        };
        let remember = self.authentication == Authentication::D4cKey;
        let fingerprint = self.saved.iter().find(|row| row.endpoint == endpoint)
            .map(|row| row.fingerprint.clone());
        let connection = Connection::start(endpoint, credential, fingerprint.clone(), ctx)?;
        self.error.clear();
        self.pending = Some(Pending { connection, verify: None, remember, fingerprint });
        Ok(())
    }
}

impl Connection {
    fn start(
        endpoint: Endpoint,
        credential: Credential,
        fingerprint: Option<String>,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let (tx, events) = mpsc::channel();
        let (stop, mut cancelled) = oneshot::channel();
        let (commands, mut inbox) = tokio::sync::mpsc::unbounded_channel();
        let report = Reporter {
            tx,
            ctx: ctx.clone(),
        };
        let worker_endpoint = endpoint.clone();
        std::thread::Builder::new().name("d4c-ssh".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all().build()
                .map_err(|error| format!("Couldn't start SSH: {error}"))
                .and_then(|runtime| runtime.block_on(async {
                    let connection_report = Reporter { tx: report.tx.clone(), ctx: report.ctx.clone() };
                    let session = tokio::select! {
                        _ = &mut cancelled => return Ok(()),
                        session = connect(worker_endpoint, credential, connection_report, fingerprint) => session?,
                    };
                    report.send(Event::Connected);
                    tokio::select! {
                        _ = &mut cancelled => {
                            let _ = session.disconnect(russh::Disconnect::ByApplication, "", "en").await;
                            Ok(())
                        }
                        result = codex::serve(&session, &mut inbox, &report) => result,
                    }
                }));
            report.send(Event::Disconnected(result.err().unwrap_or_else(|| "Disconnected".into())));
        }).map_err(|error| format!("Couldn't start SSH: {error}"))?;
        Ok(Self {
            endpoint,
            events,
            _stop: stop,
            status: CONNECTING.into(),
            codex: codex::Status::default(),
            redirect_input: String::new(),
            chat: None,
            commands,
        })
    }
}

fn quota_duration(minutes: u64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) if hours % 24 == 0 => format!("{}d", hours / 24),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

impl Connection {
    fn quota_ui(&self, ui: &mut egui::Ui, shared_count: usize, compact: bool) {
        let Some(quota) = &self.codex.quota else {
            if compact {
                ui.small("—").on_hover_text("Usage is unavailable for this server.");
            } else {
                ui.weak("Codex usage unavailable");
            }
            return;
        };
        let live = self.status == "Connected" && self.codex.quota_live;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        for (id, bucket) in &quota.buckets {
            if id != "codex" {
                let name = bucket.limit_name.as_deref().unwrap_or(id);
                if compact {
                    ui.small(name);
                } else {
                    ui.weak(name);
                }
            }
            for (index, window) in bucket.windows() {
                let name = match window.window_duration_mins {
                    Some(minutes) => quota_duration(minutes),
                    None if index == 0 => "Primary".into(),
                    None => "Secondary".into(),
                };
                let remaining = window.remaining();
                let label = format!("{name}: {remaining}% left");
                let reset = window.resets_at.filter(|_| live).map(|reset| {
                    if reset > now {
                        let time = quota_duration((reset - now).div_ceil(60));
                        format!("Resets in {time}")
                    } else {
                        "Awaiting refresh".into()
                    }
                });
                if compact {
                    let mut hint = reset.unwrap_or_default();
                    if shared_count > 1 {
                        if !hint.is_empty() {
                            hint.push('\n');
                        }
                        hint.push_str(&format!("Account quota shared by {shared_count} servers"));
                    }
                    ui.small(label).on_hover_text(hint);
                } else {
                    let label = match reset {
                        Some(reset) => format!("{label} · {reset}"),
                        None => label,
                    };
                    ui.strong(label);
                }
            }
        }
        if !compact && shared_count > 1 {
            ui.weak(format!("Shared by {shared_count} servers"));
        }
        if !live {
            let age = quota.checked.elapsed().unwrap_or_default().as_secs();
            let checked = if age < 60 {
                "just now".into()
            } else {
                format!("{} ago", quota_duration(age / 60))
            };
            let detail = format!("Last checked {checked}");
            if compact {
                ui.small("Last known").on_hover_text(detail);
            } else {
                ui.weak(detail);
            }
        }
    }

    fn codex_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut open = false;
        let mut command = None;
        if let Some(account) = self
            .codex
            .account
            .as_ref()
            .filter(|_| !self.codex.unavailable)
        {
            ui.weak(account);
        } else {
            ui.weak(format!("Codex · {}", self.codex.label()));
        }
        if self.codex.account.is_some() && !self.codex.unavailable {
            open = ui
                .small_button("Open Codex")
                .on_hover_text(self.codex.label())
                .clicked();
        } else if self.codex.unavailable {
            if ui.small_button("Retry Codex").clicked() {
                command = Some(codex::Command::Retry);
            }
        } else if !self.codex.busy {
            if ui.small_button("Sign in with browser").clicked() {
                command = Some(codex::Command::Login(false));
            }
            if ui.small_button("Use device code").clicked() {
                command = Some(codex::Command::Login(true));
            }
        } else {
            ui.spinner();
            if self.codex.can_cancel && ui.small_button("Cancel sign-in").clicked() {
                command = Some(codex::Command::Cancel);
            }
        }
        if let Some(command) = command {
            self.redirect_input.clear();
            let _ = self.commands.send(command);
        }
        open
    }

    fn codex_details(&mut self, ui: &mut egui::Ui) {
        if let Some((url, code)) = &self.codex.device {
            ui.horizontal_wrapped(|ui| {
                ui.monospace(code);
                if ui.small_button("Copy code").clicked() {
                    ui.ctx().copy_text(code.clone());
                }
                if ui.button("Open device sign-in").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            });
        }
        if self.codex.browser_callback {
            ssh_field(
                ui,
                "Paste redirect URL",
                Input::new(&mut self.redirect_input)
                    .hint("http://localhost:…/auth/callback?…")
                    .password(true)
                    .max_width(f32::INFINITY),
            );
            ui.weak("If the browser can’t return to D4C, paste the full URL from its address bar.");
            if ui
                .add_enabled(
                    !self.redirect_input.trim().is_empty(),
                    egui::Button::new("Complete sign-in"),
                )
                .clicked()
            {
                let url = std::mem::take(&mut self.redirect_input);
                if self
                    .commands
                    .send(codex::Command::SubmitRedirect(url))
                    .is_ok()
                {
                    self.codex.error = None;
                }
            }
        }
        if let Some(error) = &self.codex.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
}

#[cfg(test)]
mod tests;
