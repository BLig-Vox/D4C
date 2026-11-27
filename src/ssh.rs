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

#[derive(Clone, PartialEq, Eq)]
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
) -> Result<client::Handle<HostKey>, String> {
    let config = client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 2,
        ..Default::default()
    };
    let handler = HostKey {
        report,
        accepted: None,
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
}

struct Pending {
    connection: Connection,
    verify: Option<(String, oneshot::Sender<bool>)>,
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
    error: String,
}

fn ssh_field(ui: &mut egui::Ui, label: &str, input: Input<'_>) {
    let label = ui.label(label);
    ui.add(input).labelled_by(label.id);
}

impl Servers {
    pub fn poll(&mut self, ctx: &egui::Context) {
        for server in &mut self.servers {
            match server.events.try_recv() {
                Ok(Event::Disconnected(message)) => server.status = message,
                Err(mpsc::TryRecvError::Disconnected) if server.status == "Connected" => {
                    server.status = "Disconnected".into();
                }
                _ => {}
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
                let mut server = self.pending.take().unwrap().connection;
                server.status = "Connected".into();
                self.servers.retain(|row| row.endpoint != server.endpoint);
                self.servers.push(server);
                ctx.request_repaint();
            }
            Ok(Event::Disconnected(message)) => {
                self.error = message;
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = "SSH connection ended.".into();
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui) {
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
                        let (_, reply) = pending.verify.take().unwrap();
                        let _ = reply.send(true);
                        pending.connection.status = "Authenticating…".into();
                    }
                    ui.button("Cancel").clicked()
                })
                .inner;
            if cancel {
                self.pending = None;
            }
            return;
        }
        if self.rows_ui(ui, "") > 0 {
            ui.separator();
        }
        if ui.available_width() >= 480.0 {
            ui.columns(2, |columns| {
                self.endpoint_fields(&mut columns[0]);
                self.authentication_fields(&mut columns[1]);
            });
        } else {
            self.endpoint_fields(ui);
            self.authentication_fields(ui);
        }

        if (self.authentication != Authentication::D4cKey || self.public_key.is_empty())
            && ui
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
        if self.authentication == Authentication::D4cKey && !self.public_key.is_empty() {
            ui.separator();
            if ui
                .add(
                    Button::new(if self.key_copied {
                        "Copied public key"
                    } else {
                        "Copy public key"
                    })
                    .variant(ButtonVariant::Outline),
                )
                .clicked()
            {
                ui.copy_text(self.public_key.clone());
                self.key_copied = true;
            }
            ui.weak("Add the key to your server account’s authorized_keys before connecting.");
            egui::CollapsingHeader::new("Public key and setup details").show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&self.public_key).monospace()).wrap());
                ui.weak("Linux: ~/.ssh/authorized_keys. Windows OpenSSH: see AuthorizedKeysFile in the server configuration.");
            });
        }
        let can_connect =
            self.authentication != Authentication::D4cKey || !self.public_key.is_empty();
        if ui
            .add_enabled(can_connect, Button::new("Connect over SSH"))
            .clicked()
        {
            if let Err(error) = self.start(ui.ctx()) {
                self.error = error;
            }
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
        let mut count = 0;
        let mut remove = None;
        for (index, server) in self.servers.iter().enumerate() {
            let label = server.endpoint.to_string();
            if !label.to_lowercase().contains(query) {
                continue;
            }
            count += 1;
            ui.horizontal_wrapped(|ui| {
                ui.strong(label);
                ui.weak(format!("SSH · {}", server.status));
                if ui
                    .small_button(if server.status == "Connected" {
                        "Disconnect"
                    } else {
                        "Remove"
                    })
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            self.servers.remove(index);
        }
        count
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
            .any(|row| row.endpoint == endpoint && row.status == "Connected")
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
        let (tx, events) = mpsc::channel();
        let (stop, mut cancelled) = oneshot::channel();
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
                    let mut session = tokio::select! {
                        _ = &mut cancelled => return Ok(()),
                        session = connect(worker_endpoint, credential, connection_report) => session?,
                    };
                    report.send(Event::Connected);
                    tokio::select! {
                        _ = &mut cancelled => {
                            let _ = session.disconnect(russh::Disconnect::ByApplication, "", "en").await;
                            Ok(())
                        }
                        result = &mut session => result.map_err(|error| format!("SSH connection lost: {error}")),
                    }
                }));
            report.send(Event::Disconnected(result.err().unwrap_or_else(|| "Disconnected".into())));
        }).map_err(|error| format!("Couldn't start SSH: {error}"))?;
        self.error.clear();
        self.pending = Some(Pending {
            connection: Connection {
                endpoint,
                events,
                _stop: stop,
                status: "Connecting…".into(),
            },
            verify: None,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
