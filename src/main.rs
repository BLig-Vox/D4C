mod activity;
#[cfg(target_os = "android")]
mod android_notifications;
#[cfg(target_os = "android")]
mod android_text;
#[cfg(target_os = "android")]
mod android_uploads;
mod commands;
mod cron;
mod desktops;
mod draft_sync;
mod gateway;
mod media;
mod notifications;
mod platform;
mod preferences;
mod questions;
mod rfb;
mod servers;
mod uploads;
mod workspace;
use base64::Engine as _;
use eframe::egui;
use serde_json::{Value, json};

fn main() -> eframe::Result {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("D4C")
            .with_app_id("site.blig.D4C")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../assets/icons/256x256/site.blig.D4C.png"
                ))
                .expect("packaged D4C icon"),
            )
            .with_fullscreen(true),
        ..Default::default()
    };
    run(options)
}
fn run(options: eframe::NativeOptions) -> eframe::Result {
    let _ = rustls::crypto::ring::default_provider().install_default();
    eframe::run_native(
        "D4C",
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "lucide".into(),
                egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES).into(),
            );
            fonts.families.insert(
                egui::FontFamily::Name("lucide".into()),
                vec!["lucide".into()],
            );
            platform::install_symbol_fonts(&mut fonts);
            cc.egui_ctx.set_fonts(fonts);
            #[cfg(target_os = "android")]
            android_text::install_emoji(&cc.egui_ctx);
            let mut app = D4c::default();
            app.restore_server_drafts();
            app.reconnect(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
struct ChatMessage {
    media: Vec<media::Attachment>,
    role: String,
    text: String,
    item_id: Option<String>,
}
#[derive(Default)]
struct Draft {
    input: String,
    attachments: Vec<uploads::Attachment>,
}
struct D4c {
    desktops: desktops::Desktops,
    servers: servers::Catalogs,
    explicit_selection: bool,
    jump_latest: bool,
    workspace_hydrated: bool,
    draft_queue: draft_sync::Queue,
    imported_notifications: std::collections::HashSet<String>,
    remote_notifications: std::collections::HashSet<String>,
    workspace_sync: std::time::Instant,
    sidebar_open: bool,
    settings_open: bool,
    notifications_open: bool,
    preferences: preferences::Settings,
    center: activity::Center,
    notifier: notifications::Notifier,
    live: std::collections::HashMap<String, activity::Live>,
    connection_name: String,
    connection_url: String,
    settings_error: String,
    switch_pending: Option<usize>,
    compact_previous: bool,
    show_cron: bool,
    workspace_open: bool,
    task_chat: bool,
    cron: cron::Tasks,
    url: String,
    token: String,
    gateway: Option<gateway::Gateway>,
    status: String,
    connected: bool,
    session: String,
    sessions: Vec<(String, String)>,
    session_search: String,
    deleted_sessions: std::collections::HashSet<String>,
    rename: Option<(String, String)>,
    rename_pending: Option<String>,
    rename_error: String,
    fallback_titles: std::collections::HashMap<String, String>,
    title_requests: std::collections::HashMap<String, String>,
    media_auth: Option<String>,
    media_requests: std::collections::HashMap<String, (String, usize, usize)>,
    messages: Vec<ChatMessage>,
    input: String,
    questions: questions::Questions,
    delete_chat: Option<(String, String)>,
    delete_pending: Option<(String, String)>,
    delete_error: String,
    selection_pending: std::collections::HashMap<String, String>,
    pending_sends: std::collections::HashMap<String, Value>,
    selection_edited: std::collections::HashSet<String>,
    defaults_dirty: bool,
    model: String,
    models: Vec<String>,
    thinking: String,
    running: bool,
    attachments: Vec<(String, String, Vec<u8>)>,
    markdown_cache: egui_commonmark::CommonMarkCache,
    commands: Vec<commands::Command>,
    command_request: Option<String>,
    command_status: String,
    slash_selected: usize,
    slash_dismissed: bool,
    focus_composer: bool,
    history_request: Option<(String, String)>,
    history_loading: bool,
    history_dirty: bool,
    older_cursor: Option<String>,
    history_initialized: bool,
    older_error: String,
    older_request: Option<(String, String)>,
    history_scroll: (f32, f32),
    prepend_anchor: Option<(f32, f32)>,
    restore_scroll: Option<f32>,
    session_settings: std::collections::HashMap<String, (String, String)>,
    active_sessions: std::collections::HashSet<String>,
    drafts: std::collections::HashMap<String, Draft>,
    upload: Option<(String, std::sync::mpsc::Receiver<uploads::UploadResult>)>,
    composer_error: String,
    list_pending: bool,
    list_dirty: bool,
}
impl Default for D4c {
    fn default() -> Self {
        let preferences = preferences::Settings::load();
        let url = preferences.current().url.clone();
        let center = activity::Center::load(&url);
        let servers = servers::Catalogs::default();
        let remembered = servers.rows.get(&url).and_then(|row| row.selected.clone());
        let (default_model, default_thinking) = preferences.defaults(&url);
        Self {
            desktops: Default::default(),
            servers,
            explicit_selection: remembered.is_some(),
            jump_latest: false,
            workspace_hydrated: false,
            draft_queue: Default::default(),
            imported_notifications: Default::default(),
            remote_notifications: Default::default(),
            workspace_sync: std::time::Instant::now(),
            settings_open: url.is_empty(),
            notifications_open: false,
            preferences,
            center,
            notifier: notifications::Notifier::default(),
            live: Default::default(),
            connection_name: String::new(),
            connection_url: String::new(),
            settings_error: String::new(),
            switch_pending: None,
            compact_previous: false,
            sidebar_open: true,
            show_cron: false,
            workspace_open: false,
            task_chat: false,
            cron: cron::Tasks::default(),
            url,
            token: String::new(),
            gateway: None,
            status: "Not connected".into(),
            connected: false,
            session: remembered
                .unwrap_or_else(|| format!("agent:main:d4c-{}", uuid::Uuid::new_v4())),
            sessions: vec![],
            session_search: String::new(),
            deleted_sessions: Default::default(),
            rename: None,
            rename_pending: None,
            rename_error: String::new(),
            fallback_titles: Default::default(),
            title_requests: Default::default(),
            media_auth: None,
            media_requests: Default::default(),
            messages: vec![],
            input: String::new(),
            questions: Default::default(),
            delete_chat: None,
            delete_pending: None,
            delete_error: String::new(),
            selection_pending: Default::default(),
            pending_sends: Default::default(),
            selection_edited: Default::default(),
            defaults_dirty: false,
            model: default_model,
            models: vec![],
            thinking: default_thinking,
            running: false,
            attachments: Vec::new(),
            markdown_cache: egui_commonmark::CommonMarkCache::default(),
            commands: vec![],
            command_request: None,
            command_status: "Connect to discover commands".into(),
            slash_selected: 0,
            slash_dismissed: false,
            focus_composer: false,
            history_request: None,
            history_loading: false,
            history_dirty: false,
            older_cursor: None,
            history_initialized: false,
            older_error: String::new(),
            older_request: None,
            history_scroll: (0.0, 0.0),
            prepend_anchor: None,
            restore_scroll: None,
            session_settings: Default::default(),
            active_sessions: Default::default(),
            drafts: Default::default(),
            upload: None,
            composer_error: String::new(),
            list_pending: false,
            list_dirty: false,
        }
    }
}
impl D4c {
    fn reconnect(&mut self, ctx: &egui::Context) {
        self.workspace_hydrated = false;
        self.questions.disconnected();
        self.selection_pending.clear();
        self.selection_edited.clear();
        self.delete_chat = None;
        self.delete_pending = None;
        (self.model, self.thinking) = self.preferences.defaults(&self.url);
        if !preferences::valid_url(&self.url) {
            self.settings_open = true;
            self.status = "Add your OpenClaw server in Settings".into();
            return;
        }
        self.desktops.connect(&self.url, ctx);
        self.gateway = None;
        self.media_auth = None;
        self.media_requests.clear();
        self.connected = false;
        self.status = "Connecting…".into();
        self.cron.disconnected();
        self.command_request = None;
        self.history_request = None;
        self.older_request = None;
        self.list_pending = false;
        self.list_dirty = false;
        self.gateway = Some(gateway::connect(
            self.url.clone(),
            std::mem::take(&mut self.token),
            ctx.clone(),
        ));
    }
    fn reset_connection(&mut self, index: usize, token: String) {
        self.deleted_sessions.clear();
        self.pending_sends.clear();
        let mut preferences = self.preferences.clone();
        preferences.selected = index;
        let servers = std::mem::take(&mut self.servers);
        let remembered = servers
            .rows
            .get(&preferences.current().url)
            .and_then(|row| row.selected.clone());
        let mut next = Self::default();
        next.servers = servers;
        next.session =
            remembered.unwrap_or_else(|| format!("agent:main:d4c-{}", uuid::Uuid::new_v4()));
        next.explicit_selection = true;
        next.url = preferences.current().url.clone();
        next.center = activity::Center::load(&next.url);
        next.preferences = preferences;
        next.token = token;
        next.sidebar_open = self.sidebar_open;
        next.compact_previous = self.compact_previous;
        next.settings_open = true;
        next.restore_server_drafts();
        *self = next;
    }
    fn save_server_drafts(&mut self) -> std::io::Result<()> {
        for (_, params) in std::mem::take(&mut self.pending_sends) {
            self.restore_unsent(&params);
        }
        self.sync_drafts();
        let mut rows = Vec::new();
        let mut add = |key: &str, text: &str, attachments: &Vec<uploads::Attachment>| {
            if text.is_empty() && attachments.is_empty() {
                return;
            }
            let remote = self.desktops.workspace.get("draft", key);
            rows.push(json!({"key":key,"text":text,"base":remote.and_then(|v|v["text"].as_str()).unwrap_or(""),
                "rev":remote.and_then(|v|v["rev"].as_u64()).unwrap_or(0),
                "files":attachments.iter().map(|(name,mime,data)|json!({"name":name,"mime":mime,"data":base64::engine::general_purpose::STANDARD.encode(data)})).collect::<Vec<_>>()}));
        };
        add(&self.session, &self.input, &self.attachments);
        for (key, draft) in &self.drafts {
            if key != &self.session {
                add(key, &draft.input, &draft.attachments);
            }
        }
        let value = json!(rows);
        if value.to_string().len() > 32 * 1024 * 1024 {
            return Err(std::io::Error::other(
                "Draft attachments exceed the local backup limit",
            ));
        }
        preferences::write(&server_draft_file(&self.url), &value)
    }
    fn restore_server_drafts(&mut self) {
        for item in preferences::read(&server_draft_file(&self.url))
            .as_array()
            .into_iter()
            .flatten()
            .take(2000)
        {
            let Some(key) = item["key"].as_str().filter(|key| key.len() <= 512) else {
                continue;
            };
            let text = item["text"].as_str().unwrap_or("");
            if text.len() > 256 * 1024 {
                continue;
            }
            let mut attachments = Vec::new();
            let mut bytes = 0usize;
            for file in item["files"].as_array().into_iter().flatten().take(16) {
                let (Some(name), Some(mime), Some(data)) = (
                    file["name"].as_str(),
                    file["mime"].as_str(),
                    file["data"].as_str(),
                ) else {
                    continue;
                };
                if data.len() > 16 * 1024 * 1024 {
                    continue;
                }
                if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(data) {
                    bytes += data.len();
                    if bytes > 12 * 1024 * 1024 {
                        break;
                    }
                    attachments.push((name.to_owned(), mime.to_owned(), data));
                }
            }
            self.draft_queue.restore(
                key,
                text,
                item["base"].as_str().unwrap_or(""),
                item["rev"].as_u64().unwrap_or(0),
            );
            if key == self.session {
                self.input = text.into();
                self.attachments = attachments;
            } else {
                self.drafts.insert(
                    key.into(),
                    Draft {
                        input: text.into(),
                        attachments,
                    },
                );
            }
        }
    }
    fn switch_with_token(&mut self, index: usize, ctx: &egui::Context, token: String) {
        if index >= self.preferences.connections.len() {
            return;
        }
        if let Err(error) = self.save_server_drafts() {
            self.settings_error = format!("Could not preserve drafts: {error}");
            self.composer_error = self.settings_error.clone();
            return;
        }
        self.servers.select(&self.url, &self.session);
        self.servers.save();
        self.center.save();
        let previous = self.preferences.selected;
        self.preferences.selected = index;
        if let Err(e) = self.preferences.save() {
            self.preferences.selected = previous;
            self.settings_error = e.to_string();
            return;
        }
        self.reset_connection(index, token);
        self.reconnect(ctx);
    }
    fn switch_connection(&mut self, index: usize, ctx: &egui::Context) {
        self.switch_with_token(index, ctx, String::new());
    }
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut switch = None;
        let mut forget = None;
        egui::ScrollArea::vertical().id_salt("settings").show(ui,|ui|{
            ui.set_max_width(ui.available_width().min(640.0));
            ui.spacing_mut().interact_size.y=32.0;
            ui.heading("Settings");ui.add_space(18.0);ui.strong("Connections");ui.add_space(8.0);
            for (index,c) in self.preferences.connections.clone().iter().enumerate(){
                egui::Frame::new().fill(egui::Color32::from_rgb(29,31,36)).corner_radius(10).inner_margin(12).show(ui,|ui|{
                    ui.set_min_width((ui.available_width()).max(1.0));
                    ui.horizontal_wrapped(|ui|{
                        ui.strong(&c.name);
                        if index==self.preferences.selected {ui.label(egui::RichText::new(if self.connected{"Online"}else{"Offline"}).small().color(if self.connected{egui::Color32::from_rgb(154,209,182)}else{egui::Color32::GRAY}));}
                        else {
                            if ui.button("Switch").clicked(){switch=Some(index);}
                            ui.menu_button("More",|ui|{if ui.button("Remove connection").clicked(){forget=Some(index);ui.close();}});
                        }
                    });
                    ui.add(egui::Label::new(egui::RichText::new(&c.url).small().weak()).wrap());
                });ui.add_space(6.0);
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui|{
                if ui.button(if self.connected{"Reconnect"}else{"Connect"}).clicked(){self.reconnect(ui.ctx());}
                if self.gateway.is_some()&&ui.button("Disconnect").clicked(){self.gateway=None;self.connected=false;self.running=false;self.active_sessions.clear();self.live.clear();self.status="Disconnected".into();self.cron.disconnected();}
            });
            ui.label(egui::RichText::new(&self.status).small().weak());
            ui.add_space(6.0);
            ui.add(egui::TextEdit::singleline(&mut self.token).password(true).hint_text("Token, if required · never saved").desired_width(f32::INFINITY));
            ui.add_space(12.0);
            if preferences::valid_url(&self.url) {
                egui::CollapsingHeader::new("Workspace server").show(ui, |ui| {
                    let value = self.preferences.workspace_urls.entry(self.url.clone()).or_default();
                    ui.add(egui::TextEdit::singleline(value).hint_text("wss://workspace-host:8443 (optional)").desired_width(f32::INFINITY));
                    if ui.button("Save workspace URL").clicked() {
                        if value.is_empty() || preferences::valid_url(value) {
                            if let Err(e) = self.preferences.save() { self.settings_error = e.to_string(); }
                            else { self.settings_error.clear(); self.desktops.connect(&self.url, ui.ctx()); }
                        } else { self.settings_error = "Use a secure wss:// URL without credentials or query tokens".into(); }
                    }
                });
            }
            egui::CollapsingHeader::new("Add server").id_salt("add_server").show(ui,|ui|{
                ui.add(egui::TextEdit::singleline(&mut self.connection_name).hint_text("Name").desired_width(f32::INFINITY));
                ui.add(egui::TextEdit::singleline(&mut self.connection_url).hint_text("wss://your-server").desired_width(f32::INFINITY));
                if ui.button("Save connection").clicked(){
                    let url=self.connection_url.trim();let name=self.connection_name.trim();
                    self.settings_error=if !preferences::valid_url(url){"Use a secure wss:// address without credentials or query tokens".into()}else if name.is_empty(){"Give this connection a name".into()}else if self.preferences.connections.len()>=12{"Up to 12 connections are supported".into()}else if self.preferences.connections.iter().any(|c|c.url==url){"This server is already saved".into()}else{
                        self.preferences.connections.push(preferences::Connection{name:name.chars().take(60).collect(),url:url.into()});
                        match self.preferences.save(){Ok(())=>{self.connection_name.clear();self.connection_url.clear();String::new()},Err(e)=>e.to_string()}
                    };
                }
            });
            ui.add_space(18.0);ui.separator();ui.add_space(12.0);ui.strong("Notifications");
            let a=ui.checkbox(&mut self.preferences.desktop,"Desktop alerts").changed();
            let b=ui.checkbox(&mut self.preferences.sound,"Sound").changed();
            if a||b{if let Err(e)=self.preferences.save(){self.settings_error=e.to_string();}}
            ui.label(egui::RichText::new("Alerts appear on the device running D4C, while the app is connected.").small().weak());
            if ui.button("Test notification").clicked(){
                if self.center.push(json!({"id":format!("test:{}",activity::now()/1000),"title":"D4C","body":"Notifications are ready","session":"","time":activity::now(),"kind":"test","unread":true})) {
                    self.notifier.test(self.preferences.desktop,self.preferences.sound);
                }
            }
            ui.label(egui::RichText::new(&self.notifier.status).small().weak());
            for e in [&self.settings_error,&self.center.error]{if !e.is_empty(){ui.colored_label(egui::Color32::LIGHT_RED,e);}}
        });
        if let Some(index) = forget {
            let previous = self.preferences.clone();
            self.preferences.connections.remove(index);
            if self.preferences.selected > index {
                self.preferences.selected -= 1;
            }
            if let Err(e) = self.preferences.save() {
                self.preferences = previous;
                self.settings_error = e.to_string();
            }
        }
        if let Some(index) = switch {
            if !self.active_sessions.is_empty() {
                self.switch_pending = Some(index);
            } else {
                self.switch_connection(index, ui.ctx());
            }
        }
    }
    fn notification_ui(&mut self, ui: &mut egui::Ui) {
        let unread = self.center.unread(None);
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(icon_text(lucide_icons::Icon::ArrowLeft, 18.0))
                        .frame(false)
                        .min_size(egui::vec2(36.0, 40.0)),
                )
                .on_hover_text("Back to chat")
                .clicked()
            {
                self.notifications_open = false;
            }
            ui.add_sized(
                [(ui.available_width() - 44.0).max(40.0), 40.0],
                egui::Label::new(egui::RichText::new("Notifications").size(16.0).strong())
                    .truncate(),
            );
            if ui
                .add_enabled(
                    unread > 0,
                    egui::Button::new(icon_text(lucide_icons::Icon::CheckCheck, 18.0))
                        .frame(false)
                        .min_size(egui::vec2(36.0, 40.0)),
                )
                .on_hover_text("Mark all read")
                .clicked()
            {
                self.center.read(None);
                self.desktops.workspace.rpc("notification.read", json!({}));
            }
        });
        ui.label(
            egui::RichText::new(if unread > 0 {
                format!("{unread} unread")
            } else {
                "All caught up".into()
            })
            .size(11.0)
            .weak(),
        );
        ui.separator();
        ui.add_space(2.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("notifications")
            .show(ui, |ui| {
                if self.center.entries.is_empty() {
                    ui.add_space(48.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            icon_text(lucide_icons::Icon::Bell, 30.0)
                                .color(egui::Color32::from_gray(135)),
                        );
                        ui.add_space(10.0);
                        ui.strong("You're all caught up");
                        ui.label(
                            egui::RichText::new("Replies and scheduled results appear here.")
                                .small()
                                .weak(),
                        );
                    });
                }
                for entry in self.center.entries.clone() {
                    ui.push_id(entry["id"].as_str().unwrap_or("notification"), |ui| {
                        let is_unread = entry["unread"] == true;
                        let session = entry["session"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned);
                        let job = entry["job"].as_str().map(str::to_owned);
                        let title = entry["title"].as_str().unwrap_or("D4C");
                        let response = egui::Frame::new()
                            .fill(if is_unread {
                                egui::Color32::from_rgb(29, 39, 39)
                            } else {
                                egui::Color32::from_rgb(25, 27, 31)
                            })
                            .corner_radius(10)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        icon_text(
                                            if entry["kind"] == "cron" {
                                                lucide_icons::Icon::CalendarClock
                                            } else {
                                                lucide_icons::Icon::MessageCircle
                                            },
                                            15.0,
                                        )
                                        .color(egui::Color32::from_rgb(153, 197, 184)),
                                    );
                                    ui.add_sized(
                                        [(ui.available_width() - 88.0).max(20.0), 24.0],
                                        egui::Label::new(
                                            egui::RichText::new(title).size(13.0).strong(),
                                        )
                                        .truncate(),
                                    );
                                    let age = activity::now()
                                        .saturating_sub(entry["time"].as_u64().unwrap_or(0))
                                        / 1000;
                                    ui.label(
                                        egui::RichText::new(if age < 60 {
                                            "Now".into()
                                        } else if age < 3600 {
                                            format!("{}m", age / 60)
                                        } else if age < 86400 {
                                            format!("{}h", age / 3600)
                                        } else {
                                            format!("{}d", age / 86400)
                                        })
                                        .size(10.0)
                                        .weak(),
                                    );
                                    if is_unread {
                                        let (r, _) = ui.allocate_exact_size(
                                            egui::vec2(5.0, 5.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(
                                            r.center(),
                                            2.5,
                                            egui::Color32::from_rgb(132, 203, 177),
                                        );
                                    }
                                    ui.label(
                                        icon_text(lucide_icons::Icon::ChevronRight, 14.0)
                                            .color(egui::Color32::from_gray(128)),
                                    );
                                });
                                let body = entry["body"].as_str().unwrap_or("");
                                let plain = body
                                    .replace('`', "")
                                    .replace('*', "")
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                let mut preview: String = plain.chars().take(110).collect();
                                if plain.chars().count() > 110 {
                                    preview.push('…');
                                }
                                let mut job_layout = egui::text::LayoutJob::simple(
                                    preview,
                                    egui::FontId::proportional(12.0),
                                    egui::Color32::from_gray(192),
                                    ui.available_width(),
                                );
                                job_layout.wrap.max_rows = 2;
                                ui.label(job_layout);
                            });
                        if ui
                            .interact(
                                response.response.rect,
                                ui.id().with("open_notification"),
                                egui::Sense::click(),
                            )
                            .on_hover_text("Open conversation")
                            .clicked()
                        {
                            if let Some(session) = session {
                                self.select_session(session, true);
                                if self.compact_previous {
                                    self.sidebar_open = false;
                                }
                            }
                            if let Some(job) = job {
                                self.cron.show_history(&job, title);
                                self.settings_open = false;
                                self.notifications_open = false;
                            }
                        }
                        ui.add_space(6.0);
                    });
                }
            });
    }
    fn rpc(&self, method: &str, params: Value) -> Option<String> {
        let g = self.gateway.as_ref()?;
        let request = gateway::rpc(method, params);
        let id = request["id"].as_str()?.to_owned();
        g.tx.send(request).ok()?;
        Some(id)
    }
    fn refresh_sessions(&mut self) {
        if self.list_pending {
            self.list_dirty = true;
            return;
        }
        self.list_pending = self
            .rpc(
                "sessions.list",
                json!({"includeDerivedTitles":true,"limit":50}),
            )
            .is_some();
    }
    fn refresh_commands(&mut self) {
        self.commands.clear();
        self.command_status = "Discovering commands and skills…".into();
        let mut params = json!({"scope":"text","includeArgs":true});
        if self.sessions.iter().any(|(key, _)| key == &self.session) {
            params["sessionKey"] = json!(self.session);
        }
        self.command_request = self.rpc("commands.list", params);
    }
    fn chat_title(&self) -> &str {
        self.sessions
            .iter()
            .find(|(key, _)| key == &self.session)
            .map(|(_, title)| title.as_str())
            .or_else(|| self.fallback_titles.get(&self.session).map(String::as_str))
            .unwrap_or("New chat")
    }
    fn select_session(&mut self, key: String, load: bool) {
        self.select_session_impl(key, load, true);
    }
    fn select_session_impl(&mut self, key: String, load: bool, save_previous: bool) {
        self.jump_latest = false;
        if self.deleted_sessions.contains(&key) {
            return;
        }
        self.explicit_selection = true;
        self.servers.select(&self.url, &key);
        self.desktops.open = false;
        self.show_cron = false;
        self.workspace_open = false;
        self.settings_open = false;
        self.notifications_open = false;
        self.task_chat = false;
        self.center.read(Some(&key));
        self.desktops
            .workspace
            .rpc("notification.read", json!({"session":key}));
        if self.session == key && load {
            return;
        }
        if save_previous {
            self.sync_drafts();
            self.drafts.insert(
                self.session.clone(),
                Draft {
                    input: std::mem::take(&mut self.input),
                    attachments: std::mem::take(&mut self.attachments),
                },
            );
        } else {
            self.input.clear();
            self.attachments.clear();
        }
        self.rpc("sessions.messages.unsubscribe", json!({"key":self.session}));
        self.session = key;
        self.rpc("sessions.messages.subscribe", json!({"key":self.session}));
        self.history_dirty = false;
        let draft = self.drafts.remove(&self.session).unwrap_or_default();
        self.input = draft.input;
        if self.input.is_empty() {
            self.input = self
                .desktops
                .workspace
                .get("draft", &self.session)
                .and_then(|v| v["text"].as_str())
                .unwrap_or("")
                .to_string();
        }
        if self.workspace_hydrated {
            let rev = self
                .desktops
                .workspace
                .get("preference", "selection")
                .and_then(|v| v["rev"].as_u64())
                .unwrap_or(0);
            self.desktops.workspace.rpc("workspace.put",json!({"kind":"preference","id":"selection","expectedRevision":rev,"value":{"session":self.session}}));
        }
        self.attachments = draft.attachments;
        self.messages.clear();
        self.older_cursor = None;
        self.history_initialized = false;
        self.older_error.clear();
        self.older_request = None;
        self.prepend_anchor = None;
        self.restore_scroll = None;
        self.history_scroll = (0.0, 0.0);
        self.media_requests.clear();
        self.composer_error.clear();
        self.running = self.active_sessions.contains(&self.session);
        self.history_request = None;
        self.history_loading = false;
        (self.model, self.thinking) = self.preferences.defaults(&self.url);
        if let Some((model, thinking)) = self.session_settings.get(&self.session) {
            self.model = model.clone();
            self.thinking = thinking.clone();
        }
        if !load {
            self.patch_selection();
        }
        if load {
            if let Some(id) = self.rpc(
                "chat.history",
                json!({"sessionKey":self.session,"limit":100,"toolResultMaxChars":2000}),
            ) {
                self.history_request = Some((id, self.session.clone()));
                self.history_loading = true;
            }
        }
        self.slash_selected = 0;
        self.slash_dismissed = false;
        self.focus_composer = true;
        self.refresh_commands();
    }
    fn poll_upload(&mut self) {
        let Some((key, rx)) = &self.upload else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                let key = key.clone();
                self.upload = None;
                match result {
                    Ok(files) => {
                        let target = if key == self.session {
                            &mut self.attachments
                        } else {
                            &mut self.drafts.entry(key).or_default().attachments
                        };
                        if target.iter().map(|a| a.2.len()).sum::<usize>()
                            + files.iter().map(|a| a.2.len()).sum::<usize>()
                            <= uploads::TOTAL_LIMIT
                        {
                            target.extend(files);
                        } else {
                            self.composer_error = "Attachment limit: 12 MB total".into();
                        }
                    }
                    Err(error) => self.composer_error = error,
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.upload = None;
                self.composer_error = "File picker closed unexpectedly".into();
            }
            Err(_) => {}
        }
    }
    fn events(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self
            .gateway
            .as_ref()
            .map(|g| g.rx.try_iter().take(64).collect())
            .unwrap_or_default();
        if events.len() == 64 {
            ctx.request_repaint();
        }
        for event in events {
            match event {
                gateway::Event::Status(s) => {
                    self.connected = s == "Connected";
                    self.status = s;
                    if self.connected {
                        self.rpc("sessions.messages.subscribe", json!({"key":self.session}));
                        self.refresh_commands();
                        self.cron.refresh();
                        {
                            if let Some(id) = self.rpc(
                                "chat.history",
                                json!({"sessionKey":self.session,"limit":100,"toolResultMaxChars":2000}),
                            ) {
                                self.history_request = Some((id, self.session.clone()));
                                self.history_loading = true;
                            }
                        }
                    }
                }
                gateway::Event::Error(s) => {
                    for (_, params) in std::mem::take(&mut self.pending_sends) {
                        self.restore_unsent(&params);
                    }
                    self.connected = false;
                    self.questions.disconnected();
                    self.active_sessions.clear();
                    self.live.clear();
                    self.delete_pending = None;
                    self.running = false;
                    self.status = s;
                    self.command_request = None;
                    self.rename_pending = None;
                    self.history_request = None;
                    self.older_request = None;
                    self.history_loading = false;
                    self.cron.disconnected();
                }
                gateway::Event::Data(v) => {
                    if v["event"] == "sessions.changed" && v["payload"]["reason"] == "delete" {
                        if let Some(key) = v["payload"]["sessionKey"].as_str() {
                            self.forget_deleted_session(key);
                        }
                        self.refresh_sessions();
                        continue;
                    }
                    if v["type"] == "event"
                        && v["payload"]["sessionKey"]
                            .as_str()
                            .is_some_and(|key| self.deleted_sessions.contains(key))
                    {
                        continue;
                    }
                    self.questions.observe(&v);
                    let response_id = v["id"].as_str().unwrap_or("");
                    if v["type"] == "res" {
                        if response_id.starts_with("sessions.patch:") {
                            eprintln!(
                                "D4C model response id={response_id} ok={} waiting={}",
                                v["ok"],
                                self.pending_sends.contains_key(response_id)
                            );
                        }
                        if let Some(params) = self.pending_sends.remove(response_id) {
                            if v["ok"] == true {
                                if !params["sessionKey"]
                                    .as_str()
                                    .is_some_and(|key| self.deleted_sessions.contains(key))
                                {
                                    if self.rpc("chat.send", params.clone()).is_none() {
                                        self.restore_unsent(&params);
                                    }
                                }
                            } else {
                                self.restore_unsent(&params);
                            }
                        }
                        if let Some(key) = self.selection_pending.remove(response_id) {
                            if v["ok"] == false {
                                self.selection_edited.remove(&key);
                                self.composer_error = v["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Model update failed")
                                    .into();
                                self.refresh_sessions();
                            }
                        }
                        if self
                            .delete_pending
                            .as_ref()
                            .is_some_and(|(id, _)| id == response_id)
                        {
                            let (_, target) = self.delete_pending.take().unwrap();
                            if v["ok"] == true {
                                self.forget_deleted_session(&target);
                                self.refresh_sessions();
                            } else {
                                self.delete_error = v["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Could not delete chat")
                                    .into();
                            }
                            continue;
                        }
                    }
                    let p = &v["payload"];
                    if v["id"] == "connect" && v["ok"] == true {
                        self.media_auth = p["auth"]["deviceToken"].as_str().map(str::to_owned);
                    }

                    if let Some(session) = p["sessionKey"].as_str() {
                        if v["event"] == "agent" || (v["event"] == "chat" && p["state"] == "delta")
                        {
                            if self.live.len() < 100 || self.live.contains_key(session) {
                                self.live.entry(session.into()).or_default().observe(&v);
                            }
                        }
                        if v["event"] == "chat"
                            && matches!(p["state"].as_str(), Some("final" | "error" | "aborted"))
                        {
                            self.live.remove(session);
                            let visible = session == self.session
                                && chat_is_visible(
                                    self.settings_open,
                                    self.workspace_open && ctx.content_rect().width() < 640.0,
                                    self.notifications_open,
                                    self.sidebar_open,
                                    ctx.content_rect().width() < 640.0,
                                    ctx.input(|i| i.viewport().focused.unwrap_or(false)),
                                );
                            if !visible
                                && p["state"] != "aborted"
                                && !session.contains(":cron:")
                                && !session.contains(":automation:")
                            {
                                let title = self
                                    .sessions
                                    .iter()
                                    .find(|(k, _)| k == session)
                                    .map(|(_, t)| t.as_str())
                                    .unwrap_or("Assistant reply");
                                let body = message_text(&p["message"]);
                                let run =
                                    p["runId"].as_str().map(str::to_owned).unwrap_or_else(|| {
                                        format!(
                                            "{:x}",
                                            <sha2::Sha256 as sha2::Digest>::digest(
                                                format!("{session}:{body}:{}", p["state"])
                                                    .as_bytes()
                                            )
                                        )
                                    });
                                let entry = json!({"id":format!("chat:{run}"),"title":title,"body":if body.is_empty(){"Assistant finished"}else{&body},"session":session,"time":activity::now(),"kind":"chat","unread":true});
                                if self.center.push(entry.clone()) {
                                    self.notifier.send(
                                        entry["title"].as_str().unwrap_or("D4C"),
                                        entry["body"].as_str().unwrap_or(""),
                                        self.preferences.desktop,
                                        self.preferences.sound,
                                    );
                                }
                            }
                        }
                    }
                    if self.cron.handle(&v) {
                        continue;
                    }
                    let id = v["id"].as_str().unwrap_or("");
                    if let Some((session, message, attachment)) = self.media_requests.remove(id) {
                        if session == self.session && v["ok"] == true {
                            let p = &v["payload"];
                            if let Some(encoded) =
                                p["data"].as_str().filter(|s| s.len() <= 8_000_000)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(encoded)
                                {
                                    if let Some(media) = self
                                        .messages
                                        .get_mut(message)
                                        .and_then(|m| m.media.get_mut(attachment))
                                    {
                                        media.receive(
                                            p["artifact"]["mimeType"]
                                                .as_str()
                                                .unwrap_or("image/webp")
                                                .into(),
                                            bytes,
                                        );
                                    }
                                }
                            }
                        }
                        if session == self.session
                            && v["ok"] == true
                            && v["payload"]["data"].as_str().is_none()
                        {
                            if let (Some(path), Some(token)) =
                                (v["payload"]["url"].as_str(), self.media_auth.as_ref())
                            {
                                if let Some(media) = self
                                    .messages
                                    .get_mut(message)
                                    .and_then(|m| m.media.get_mut(attachment))
                                {
                                    media.receive_url(&self.url, token, path);
                                }
                            }
                        }
                        continue;
                    }

                    if self.command_request.as_deref() == Some(id) {
                        self.command_request = None;
                        if v["ok"] == true {
                            self.commands = commands::parse(&v["payload"]);
                            self.command_status =
                                format!("{} commands and skills", self.commands.len());
                        } else {
                            self.command_status = format!(
                                "Command discovery unavailable: {}",
                                v["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Gateway request failed")
                            );
                        }
                        continue;
                    }

                    if v["type"] == "res"
                        && v["id"]
                            .as_str()
                            .is_some_and(|id| self.rename_pending.as_deref() == Some(id))
                    {
                        self.rename_pending = None;
                        if v["ok"] == true {
                            self.rename = None;
                            self.rename_error.clear();
                            self.refresh_sessions();
                        } else {
                            self.rename_error = v["error"]["message"]
                                .as_str()
                                .unwrap_or("Could not rename chat")
                                .to_owned();
                        }
                        continue;
                    }
                    if v["type"] == "res" && v["ok"] == false {
                        self.title_requests.remove(id);
                        if id.starts_with("sessions.list:") {
                            self.list_pending = false;
                            self.list_dirty = false;
                        }
                        if self
                            .history_request
                            .as_ref()
                            .is_some_and(|(req, _)| req == id)
                        {
                            self.history_request = None;
                            self.history_loading = false;
                        }

                        if self
                            .older_request
                            .as_ref()
                            .is_some_and(|(req, _)| req == id)
                        {
                            self.older_request = None;
                            self.older_error = v["error"]["message"]
                                .as_str()
                                .unwrap_or("Could not load earlier messages")
                                .into();
                        }
                        self.status = v["error"]["message"]
                            .as_str()
                            .unwrap_or("Gateway request failed")
                            .to_owned();
                        if id.starts_with("sessions.patch:") || id.starts_with("chat.send:") {
                            self.composer_error = self.status.clone();
                        }
                        if id.starts_with("chat.send:") {
                            self.running = false;
                            self.active_sessions.remove(&self.session);
                        }
                        continue;
                    }
                    let id = v["id"].as_str().unwrap_or("");
                    let payload = &v["payload"];
                    if let Some(key) = self.title_requests.remove(id) {
                        if let Some(items) = payload["messages"].as_array() {
                            if let Some(title) = items
                                .iter()
                                .filter(|m| m["role"] == "user")
                                .map(message_text)
                                .find(|s| !s.trim().is_empty())
                            {
                                let title: String = title
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ")
                                    .chars()
                                    .take(60)
                                    .collect();
                                self.fallback_titles.insert(key.clone(), title.clone());
                                if let Some((_, name)) =
                                    self.sessions.iter_mut().find(|(k, _)| k == &key)
                                {
                                    if name == "New chat" {
                                        *name = title;
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    if id.starts_with("sessions.list:") {
                        self.list_pending = false;
                        if self.list_dirty {
                            self.list_dirty = false;
                            self.refresh_sessions();
                        }
                        if let Some(items) = payload["sessions"].as_array() {
                            for row in items {
                                if let Some(key) = row["key"].as_str() {
                                    let model = row["model"].as_str().unwrap_or(&self.model);
                                    let provider =
                                        row["modelProvider"].as_str().unwrap_or("openai");
                                    let model = if model.contains('/') {
                                        model.to_owned()
                                    } else {
                                        format!("{provider}/{model}")
                                    };
                                    if !self.selection_edited.contains(key) {
                                        self.session_settings.insert(
                                            key.into(),
                                            (
                                                model,
                                                row["thinkingLevel"]
                                                    .as_str()
                                                    .unwrap_or("medium")
                                                    .into(),
                                            ),
                                        );
                                    }
                                    if row["hasActiveRun"] == true {
                                        self.active_sessions.insert(key.into());
                                    } else if row["hasActiveRun"] == false
                                        && !self.live.contains_key(key)
                                    {
                                        self.active_sessions.remove(key);
                                    }
                                }
                            }

                            if !self.selection_edited.contains(&self.session) {
                                if let Some((model, thinking)) =
                                    self.session_settings.get(&self.session)
                                {
                                    self.model = model.clone();
                                    self.thinking = thinking.clone();
                                }
                            }
                            self.sessions = items
                                .iter()
                                .filter_map(|s| {
                                    Some((
                                        {
                                            let key = s["key"].as_str()?;
                                            if self.deleted_sessions.contains(key) {
                                                return None;
                                            }
                                            key.to_string()
                                        },
                                        s["label"]
                                            .as_str()
                                            .filter(|v| !v.trim().is_empty())
                                            .or_else(|| {
                                                s["derivedTitle"]
                                                    .as_str()
                                                    .filter(|v| !v.trim().is_empty())
                                            })
                                            .or_else(|| {
                                                s["displayName"].as_str().filter(|v| {
                                                    !v.trim().is_empty() && !v.starts_with("agent:")
                                                })
                                            })
                                            .or_else(|| {
                                                s["firstUserMessage"]
                                                    .as_str()
                                                    .filter(|v| !v.trim().is_empty())
                                            })
                                            .unwrap_or("New chat")
                                            .to_string(),
                                    ))
                                })
                                .collect();
                            let missing: Vec<String> = self
                                .sessions
                                .iter_mut()
                                .filter_map(|(key, title)| {
                                    if title != "New chat" {
                                        return None;
                                    }
                                    if let Some(cached) = self.fallback_titles.get(key) {
                                        *title = cached.clone();
                                        return None;
                                    }
                                    if self.title_requests.values().any(|pending| pending == key) {
                                        return None;
                                    }
                                    Some(key.clone())
                                })
                                .collect();
                            for key in missing {
                                let req = gateway::rpc(
                                    "chat.history",
                                    json!({"sessionKey":key,"limit":100}),
                                );
                                let request_id = req["id"].as_str().unwrap_or("").to_owned();
                                if let Some(g) = &self.gateway {
                                    if g.tx.send(req).is_ok() {
                                        self.title_requests.insert(request_id, key);
                                    }
                                }
                            }
                        }
                    }
                    if v["event"] == "sessions.changed" {
                        self.refresh_sessions();
                        if payload["sessionKey"].as_str() == Some(self.session.as_str())
                            && payload["phase"] == "message"
                        {
                            self.history_dirty = true;
                        }
                    }
                    let current_event =
                        payload["sessionKey"].as_str() == Some(self.session.as_str());
                    if current_event
                        && matches!(
                            v["event"].as_str(),
                            Some("session.message" | "chat" | "agent")
                        )
                    {
                        // A background snapshot requested before this event may be stale.
                        if !self.history_loading && self.history_request.is_some() {
                            self.history_request = None;
                            self.history_dirty = true;
                        }
                    }
                    if current_event && v["event"] == "session.message" {
                        self.history_dirty = true;
                        let message = &payload["message"];
                        if message["role"] == "user" {
                            let text = message_text(message);
                            let media = media::from_message(message);
                            let canonical_id = message["__openclaw"]["id"]
                                .as_str()
                                .or_else(|| message["id"].as_str())
                                .or_else(|| payload["messageId"].as_str())
                                .map(|id| format!("message:{id}"));
                            let known = canonical_id.as_ref().is_some_and(|id| {
                                self.messages.iter().any(|m| m.item_id.as_ref() == Some(id))
                            });
                            if !known && (!text.is_empty() || !media.is_empty()) {
                                // Attach the server identity to our optimistic local echo.
                                if let Some(echo) = self.messages.iter_mut().rev().find(|m| {
                                    m.role == "user"
                                        && m.text == text
                                        && (m
                                            .item_id
                                            .as_deref()
                                            .is_some_and(|id| id.starts_with("local:"))
                                            || (self.running && m.item_id.is_none()))
                                }) {
                                    // Tool/assistant activity may already follow our local row.
                                    // A server ID identifies the echo; identical later sends still
                                    // have their own local/server IDs and remain separate messages.
                                    if canonical_id.is_some() {
                                        echo.item_id = canonical_id;
                                    }
                                    if !media.is_empty() {
                                        echo.media = media;
                                    }
                                } else {
                                    self.messages.push(ChatMessage {
                                        role: "user".into(),
                                        text,
                                        media,
                                        item_id: canonical_id,
                                    });
                                }
                            }
                        }
                    }
                    if id.starts_with("models.list:") {
                        if let Some(items) = payload["models"].as_array() {
                            let models: Vec<_> = items
                                .iter()
                                .filter_map(|m| {
                                    let id = m["id"].as_str()?;
                                    let provider = m["provider"].as_str().unwrap_or("openai");
                                    Some(if id.contains('/') {
                                        id.to_string()
                                    } else {
                                        format!("{provider}/{id}")
                                    })
                                })
                                .collect();
                            if !models.is_empty() {
                                if self.model.is_empty() {
                                    self.model = models[0].clone();
                                }
                                self.models = models;
                            }
                        }
                    }
                    let older_page = self
                        .older_request
                        .as_ref()
                        .is_some_and(|(req, key)| req == id && key == &self.session);
                    if older_page
                        || self
                            .history_request
                            .as_ref()
                            .is_some_and(|(req, key)| req == id && key == &self.session)
                    {
                        if older_page {
                            self.older_request = None;
                        } else {
                            self.history_request = None;
                            self.history_loading = false;
                        }
                        if let Some(running) = payload["sessionInfo"]["hasActiveRun"].as_bool() {
                            self.running = running;
                        }
                        let mut previous = std::mem::take(&mut self.messages);
                        if let Some(items) = payload["messages"].as_array() {
                            for m in items {
                                let tool = m["toolName"]
                                    .as_str()
                                    .or_else(|| m["name"].as_str())
                                    .unwrap_or("");
                                if cron::is_scheduler_tool(tool) {
                                    self.cron.observe_tool(&self.session, m);
                                }
                                let role = m["role"].as_str().unwrap_or("");
                                let text = message_text(m);
                                let media = media::from_message(m);
                                if (role == "user"
                                    || role == "assistant"
                                    || matches!(role, "toolResult" | "tool"))
                                    && (!text.is_empty() || !media.is_empty())
                                {
                                    self.messages.push(ChatMessage {
                                        media,
                                        role: if matches!(role, "toolResult" | "tool") {
                                            tool_title(tool, "result", m["isError"] == true)
                                        } else {
                                            role.into()
                                        },
                                        text,
                                        item_id: if matches!(role, "toolResult" | "tool") {
                                            Some(format!(
                                                "tool:{}",
                                                m["toolCallId"]
                                                    .as_str()
                                                    .or_else(|| m["__openclaw"]["id"].as_str())
                                                    .unwrap_or(tool)
                                            ))
                                        } else {
                                            m["__openclaw"]["id"]
                                                .as_str()
                                                .or_else(|| m["id"].as_str())
                                                .map(|id| format!("message:{id}"))
                                        },
                                    });
                                }
                            }
                        }
                        // Only the newest user in a fresh tail can acknowledge a
                        // pending local send. Never content-match local rows against
                        // older pages: intentional repeated prompts must survive.
                        if !older_page {
                            if let (Some(local), Some(canonical)) = (
                                previous.iter_mut().rev().find(|m| m.role == "user" &&
                                    m.item_id.as_deref().is_some_and(|id| id.starts_with("local:"))),
                                self.messages.iter().rev().find(|m| m.role == "user"),
                            ) {
                                if local.text == canonical.text && !local.text.is_empty() &&
                                    local.media.len() == canonical.media.len() &&
                                    local.media.iter().zip(&canonical.media).all(|(a,b)| a.name == b.name && a.mime == b.mime) {
                                    if canonical.item_id.is_some() { local.item_id = canonical.item_id.clone(); }
                                }
                            }
                        }
                        let page_cursor = payload["olderCursor"].as_str().map(str::to_owned);
                        if older_page {
                            self.older_cursor =
                                page_cursor.filter(|next| self.older_cursor.as_ref() != Some(next));
                            // The page precedes the existing transcript; exclude any
                            // boundary overlap using server identities, not text alone.
                            self.messages
                                .retain(|m| !previous.iter().any(|old| same_chat_message(m, old)));
                            self.prepend_anchor = Some(self.history_scroll);
                            self.messages.extend(previous);
                        } else {
                            // A tail refresh must not throw away pages the user loaded.
                            let overlap = self.messages.iter().find_map(|m| {
                                previous.iter().position(|old| same_chat_message(m, old))
                            });
                            if let Some(index) = overlap {
                                let tail = std::mem::take(&mut self.messages);
                                self.messages = previous.into_iter().take(index).collect();
                                self.messages.extend(tail);
                            } else if !previous.is_empty() {
                                // A busy turn may move the whole tail window ahead.
                                // Retain the already-read prompts rather than erase them.
                                let tail = std::mem::take(&mut self.messages);
                                self.messages = previous;
                                self.messages.extend(tail);
                            }
                            if !self.history_initialized {
                                self.older_cursor = page_cursor;
                                self.history_initialized = true;
                            }
                        }
                        self.media_requests.clear();
                        if !older_page {
                            self.recover_inflight(&payload["inFlightRun"]);
                        }
                    }
                    if v["event"] == "chat" {
                        if let Some(key) = payload["sessionKey"].as_str() {
                            if matches!(
                                payload["state"].as_str(),
                                Some("final" | "error" | "aborted")
                            ) {
                                self.active_sessions.remove(key);
                            } else {
                                self.active_sessions.insert(key.into());
                            }
                        }
                    }
                    if matches!(
                        v["event"].as_str(),
                        Some("skills.changed" | "commands.changed")
                    ) {
                        self.refresh_commands();
                    }
                    if v["event"] == "chat"
                        && payload["sessionKey"].as_str() == Some(self.session.as_str())
                    {
                        let state = payload["state"].as_str().unwrap_or("");
                        if state == "delta" || state == "final" {
                            let snapshot_media = media::from_message(&payload["message"]);
                            let snapshot = message_text(&payload["message"]);
                            let delta = payload["deltaText"].as_str().unwrap_or("");
                            let replace = payload["replace"].as_bool().unwrap_or(false);
                            if !snapshot.is_empty()
                                || !delta.is_empty()
                                || replace
                                || !snapshot_media.is_empty()
                            {
                                if !self.messages.last().is_some_and(|m| m.role == "assistant") {
                                    self.messages.push(ChatMessage {
                                        media: Default::default(),
                                        role: "assistant".into(),
                                        text: String::new(),
                                        item_id: None,
                                    });
                                }
                                if let Some(last) = self.messages.last_mut() {
                                    if !snapshot_media.is_empty() {
                                        last.media = snapshot_media;
                                    }
                                    if !snapshot.is_empty() {
                                        last.text = snapshot;
                                    } else if replace {
                                        last.text = delta.into();
                                    } else {
                                        last.text.push_str(delta);
                                    }
                                }
                            }
                            if state == "final" {
                                self.history_dirty = true;
                                self.running = false;
                                self.status = "Connected".into();
                                if let Some(last) = self.messages.last_mut() {
                                    if last.role == "assistant" {
                                        last.item_id = None;
                                    }
                                }
                                self.refresh_sessions();
                                self.cron.refresh();
                            }
                        } else if state == "error" || state == "aborted" {
                            self.running = false;
                            self.status = payload["errorMessage"].as_str().unwrap_or(state).into();
                        }
                    }
                    if v["event"] == "agent"
                        && payload["sessionKey"].as_str() == Some(self.session.as_str())
                    {
                        let stream = payload["stream"].as_str().unwrap_or("");
                        let data = &payload["data"];
                        if stream == "item" && data["kind"] == "answer_candidate" {
                            if let Some(text) = data["progressText"].as_str() {
                                let item_id = data["itemId"].as_str().unwrap_or("current");
                                let key = format!("draft:{item_id}");
                                if let Some(message) = self
                                    .messages
                                    .iter_mut()
                                    .find(|m| m.item_id.as_deref() == Some(key.as_str()))
                                {
                                    message.text = text.to_owned();
                                } else {
                                    self.messages.push(ChatMessage {
                                        media: Default::default(),
                                        role: "assistant".into(),
                                        text: text.to_owned(),
                                        item_id: Some(key),
                                    });
                                }
                            }
                        } else if stream == "item" && data["kind"] == "preamble" {
                            if let Some(text) = data["progressText"].as_str() {
                                let item_id = data["itemId"].as_str().unwrap_or("current");
                                let key = format!("preamble:{item_id}");
                                if let Some(message) = self
                                    .messages
                                    .iter_mut()
                                    .find(|m| m.item_id.as_deref() == Some(key.as_str()))
                                {
                                    message.text = text.to_owned();
                                } else {
                                    self.messages.push(ChatMessage {
                                        media: Default::default(),
                                        role: "Activity".into(),
                                        text: text.to_owned(),
                                        item_id: Some(key),
                                    });
                                }
                            }
                        } else if stream == "tool" {
                            let id = data["toolCallId"]
                                .as_str()
                                .or_else(|| data["itemId"].as_str())
                                .unwrap_or("unknown");
                            let name = data["name"].as_str().unwrap_or("Tool");
                            let phase = data["phase"].as_str().unwrap_or("update");
                            if cron::is_scheduler_tool(name)
                                && matches!(phase, "result" | "end")
                                && data["isError"] != true
                            {
                                self.cron.observe_tool(&self.session, &data["result"]);
                                self.cron.refresh();
                            }
                            let media = media::from_message(&data["result"]);
                            let detail = tool_details(data);
                            let title = tool_title(
                                name,
                                phase,
                                data["isError"] == true || data["result"]["isError"] == true,
                            );
                            let key = format!("tool:{id}");
                            let row = self
                                .messages
                                .iter()
                                .position(|m| m.item_id.as_deref() == Some(key.as_str()));
                            if let Some(i) = row {
                                let m = &mut self.messages[i];
                                if !detail.is_empty() {
                                    m.text.push_str("\n");
                                    m.text.push_str(&detail);
                                }
                                m.role = title;
                                if !media.is_empty() {
                                    m.media = media;
                                }
                            } else {
                                self.messages.push(ChatMessage {
                                    media,
                                    role: title,
                                    text: detail,
                                    item_id: Some(key),
                                });
                            }
                            self.status = if phase == "result" {
                                "Connected".into()
                            } else {
                                format!("Running {name}…")
                            };
                        } else if stream == "reasoning" || stream == "thinking" {
                            let text = data["text"]
                                .as_str()
                                .or_else(|| data["delta"].as_str())
                                .unwrap_or("");
                            if !text.is_empty() {
                                let key = data["itemId"].as_str().unwrap_or("current");
                                let key = format!("reasoning:{key}");
                                if let Some(m) = self
                                    .messages
                                    .iter_mut()
                                    .find(|m| m.item_id.as_deref() == Some(key.as_str()))
                                {
                                    if data["text"].is_string() || data["replace"] == true {
                                        m.text = text.into();
                                    } else {
                                        m.text.push_str(text);
                                    }
                                } else {
                                    self.messages.push(ChatMessage {
                                        media: Default::default(),
                                        role: "Reasoning summary".into(),
                                        text: text.into(),
                                        item_id: Some(key),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fn reconcile_history(&mut self) {
        if self.connected && self.history_dirty && !self.running && self.history_request.is_none() {
            if let Some(id) = self.rpc(
                "chat.history",
                json!({"sessionKey":self.session,"limit":100,"toolResultMaxChars":2000}),
            ) {
                self.history_dirty = false;
                self.history_request = Some((id, self.session.clone()));
            }
        }
    }
    fn sync_drafts(&mut self) {
        if !self.desktops.workspace.ready {
            return;
        }
        self.draft_queue
            .set_device(self.desktops.workspace.device.clone());
        let remote = self
            .desktops
            .workspace
            .get("draft", &self.session)
            .cloned()
            .unwrap_or(json!({"rev":0,"text":""}));
        self.input = self
            .draft_queue
            .synchronize(&self.session, &self.input, &remote);
        for (key, draft) in self.drafts.iter_mut() {
            let remote = self
                .desktops
                .workspace
                .get("draft", key)
                .cloned()
                .unwrap_or(json!({"rev":0,"text":""}));
            draft.input = self.draft_queue.synchronize(key, &draft.input, &remote);
        }
        if self.desktops.workspace.status == "Online" {
            for request in self.draft_queue.requests() {
                self.desktops.workspace.rpc("workspace.put", request);
            }
        }
        if self.draft_queue.conflicted(&self.session) {
            self.composer_error =
                "Your draft differs from the saved version. Your text is preserved here.".into();
        } else if matches!(
            self.composer_error.as_str(),
            "This draft changed on another device. Your text is preserved here."
                | "Your draft differs from the saved version. Your text is preserved here."
        ) {
            self.composer_error.clear();
        }
    }
    fn sync_workspace(&mut self) {
        if !self.desktops.workspace.ready {
            return;
        }
        if !self.workspace_hydrated {
            self.workspace_hydrated = true;
            if let Some(v) = self
                .desktops
                .workspace
                .get("preference", "model-default")
                .filter(|_| !self.defaults_dirty)
            {
                if let (Some(model), Some(thinking)) = (v["model"].as_str(), v["thinking"].as_str())
                {
                    self.preferences
                        .model_defaults
                        .insert(self.url.clone(), (model.into(), thinking.into()));
                    let _ = self.preferences.save();
                    if !self.session_settings.contains_key(&self.session) {
                        self.model = model.into();
                        self.thinking = thinking.into();
                    }
                }
            }
            self.remote_notifications = self
                .desktops
                .workspace
                .list("notification")
                .iter()
                .filter_map(|n| n["id"].as_str().map(str::to_owned))
                .collect();
            if !self.explicit_selection && self.messages.is_empty() && self.input.is_empty() {
                if let Some(key) = self
                    .desktops
                    .workspace
                    .get("preference", "selection")
                    .and_then(|v| v["session"].as_str())
                    .map(str::to_owned)
                {
                    self.select_session(key, true);
                }
            }
            if self
                .desktops
                .workspace
                .get("preference", "selection")
                .is_none()
            {
                self.desktops.workspace.rpc("workspace.put",json!({"kind":"preference","id":"selection","expectedRevision":0,"value":{"session":self.session}}));
            }
            if self.input.is_empty() {
                self.input = self
                    .desktops
                    .workspace
                    .get("draft", &self.session)
                    .and_then(|v| v["text"].as_str())
                    .unwrap_or("")
                    .into();
            }
        }
        self.sync_drafts();
        if self.workspace_sync.elapsed() < std::time::Duration::from_millis(750) {
            return;
        }
        self.workspace_sync = std::time::Instant::now();
        if self.defaults_dirty {
            let (model, thinking) = self.preferences.defaults(&self.url);
            let current = self.desktops.workspace.get("preference", "model-default");
            if current.is_some_and(|v| v["model"] == model && v["thinking"] == thinking) {
                self.defaults_dirty = false;
            } else {
                let rev = current.and_then(|v| v["rev"].as_u64()).unwrap_or(0);
                self.desktops.workspace.rpc("workspace.put",json!({"kind":"preference","id":"model-default","expectedRevision":rev,"value":{"model":model,"thinking":thinking}}));
            }
        }

        if !self.notifications_open
            && !self.settings_open
            && !(self.workspace_open && self.compact_previous)
            && self.center.unread(Some(&self.session)) == 0
            && self
                .desktops
                .workspace
                .list("notification")
                .iter()
                .any(|n| n["session"] == self.session && n["unread"] == true)
        {
            self.desktops
                .workspace
                .rpc("notification.read", json!({"session":self.session}));
        }
        // Migrate each pre-existing local alert once. Server IDs make imports idempotent.
        for entry in self.center.entries.clone() {
            if let Some(id) = entry["id"].as_str() {
                if self.imported_notifications.insert(id.into())
                    && self.desktops.workspace.get("notification", id).is_none()
                {
                    self.desktops.workspace.rpc("notification.import", entry);
                }
            }
        }
        let mut notifications = self.desktops.workspace.list("notification");
        for entry in &notifications {
            if let Some(id) = entry["id"].as_str() {
                if self.remote_notifications.insert(id.into())
                    && entry["unread"] == true
                    && !self.center.entries.iter().any(|n| n["id"] == id)
                    && entry["session"] != self.session
                {
                    #[cfg(not(target_os = "android"))]
                    self.notifier.send(
                        entry["title"].as_str().unwrap_or("D4C"),
                        entry["body"].as_str().unwrap_or(""),
                        self.preferences.desktop,
                        self.preferences.sound,
                    );
                }
            }
        }
        notifications.sort_by_key(|n| std::cmp::Reverse(n["time"].as_u64().unwrap_or(0)));
        if !notifications.is_empty() {
            self.center.entries = notifications.into_iter().take(100).collect();
        }
    }
    fn recover_inflight(&mut self, run: &Value) {
        if !run["runId"].is_string() {
            return;
        }
        self.running = true;
        self.active_sessions.insert(self.session.clone());
        let live = self.live.entry(self.session.clone()).or_default();
        for event in run["events"].as_array().into_iter().flatten() {
            live.observe(&json!({"event":"agent","payload":event}));
            if event["stream"] == "tool" {
                let data = &event["data"];
                let name = data["name"].as_str().unwrap_or("Tool");
                let phase = data["phase"].as_str().unwrap_or("start");
                self.messages.push(ChatMessage {
                    media: media::from_message(&data["result"]),
                    role: tool_title(
                        name,
                        phase,
                        data["isError"] == true || data["result"]["isError"] == true,
                    ),
                    text: tool_details(data),
                    item_id: Some(format!(
                        "tool:{}",
                        data["toolCallId"].as_str().unwrap_or("recovered")
                    )),
                });
            }
        }
        if let Some(text) = run["text"].as_str().filter(|s| !s.is_empty()) {
            self.messages.push(ChatMessage {
                media: Default::default(),
                role: "assistant".into(),
                text: text.into(),
                item_id: None,
            });
        }
    }
    fn composer(&mut self, ui: &mut egui::Ui) {
        let composer_id = egui::Id::unique("message_input");
        let focused = ui.memory(|m| m.has_focus(composer_id));
        let mut candidates = if self.slash_dismissed {
            vec![]
        } else {
            commands::suggestions(&self.commands, &self.input)
        };
        if !self.slash_dismissed {
            candidates.extend(self.desktops.suggestions(&self.input));
        }
        self.slash_selected = self.slash_selected.min(candidates.len().saturating_sub(1));
        let mut accept = None;
        let mut send = false;
        if focused {
            ui.input_mut(|i| {
                if consume_plain_key(i, egui::Key::Escape) {
                    self.slash_dismissed = true;
                    candidates.clear();
                }
                if !candidates.is_empty() {
                    if consume_plain_key(i, egui::Key::ArrowDown) {
                        self.slash_selected = (self.slash_selected + 1) % candidates.len();
                    }
                    if consume_plain_key(i, egui::Key::ArrowUp) {
                        self.slash_selected =
                            (self.slash_selected + candidates.len() - 1) % candidates.len();
                    }
                    if consume_plain_key(i, egui::Key::Tab)
                        || consume_plain_key(i, egui::Key::Enter)
                    {
                        accept = Some(self.slash_selected);
                    }
                } else if consume_plain_key(i, egui::Key::Enter) {
                    send = true;
                }
            });
        }
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(32, 33, 37))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(61)))
            .corner_radius(16)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if self.input.starts_with('/') && !self.slash_dismissed {
                    if !candidates.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Commands & skills").small().strong());
                            ui.label(
                                egui::RichText::new(
                                    "↑ ↓ to choose · Tab to insert · Esc to dismiss",
                                )
                                .small()
                                .weak(),
                            );
                        });
                        egui::ScrollArea::vertical()
                            .id_salt("slash_commands")
                            .max_height(180.0)
                            .show(ui, |ui| {
                                for (index, item) in candidates.iter().enumerate() {
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 42.0),
                                        egui::Sense::click(),
                                    );
                                    if index == self.slash_selected || response.hovered() {
                                        ui.painter().rect_filled(
                                            rect,
                                            6,
                                            egui::Color32::from_rgb(47, 49, 55),
                                        );
                                    }
                                    ui.painter().text(
                                        rect.left_center() + egui::vec2(12.0, -8.0),
                                        egui::Align2::LEFT_CENTER,
                                        &item.label,
                                        egui::FontId::proportional(13.0),
                                        egui::Color32::from_gray(235),
                                    );
                                    let desc = if item.skill {
                                        format!("Skill · {}", item.description)
                                    } else {
                                        item.description.clone()
                                    };
                                    paint_ellipsis(
                                        ui,
                                        egui::Rect::from_min_size(
                                            rect.left_top() + egui::vec2(12.0, 23.0),
                                            egui::vec2(rect.width() - 24.0, 16.0),
                                        ),
                                        &desc,
                                        11.0,
                                        egui::Color32::from_gray(155),
                                    );
                                    if response.clicked() {
                                        accept = Some(index);
                                    }
                                    if index == self.slash_selected {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                        ui.separator();
                    } else {
                        ui.horizontal(|ui| {
                            let token = self.input.split_whitespace().next().unwrap_or("");
                            let text =
                                if self.command_request.is_some() || self.commands.is_empty() {
                                    self.command_status.clone()
                                } else if let Some(command) = self.commands.iter().find(|c| {
                                    c.token == token || c.aliases.iter().any(|a| a == token)
                                }) {
                                    let args = command
                                        .args
                                        .iter()
                                        .filter_map(|a| a["name"].as_str())
                                        .map(|name| format!("<{name}>"))
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    format!("{} {} · {}", command.token, args, command.description)
                                } else {
                                    "No matching command · edit your search".into()
                                };
                            ui.label(egui::RichText::new(text).small().weak());
                            if ui.small_button("Refresh").clicked() {
                                self.refresh_commands();
                            }
                        });
                    }
                }
                if let Some(index) = accept {
                    self.input = candidates[index].replacement.clone();
                    self.slash_selected = 0;
                    self.focus_composer = true;
                    self.slash_dismissed = false;
                    let mut state =
                        egui::TextEdit::load_state(ui.ctx(), composer_id).unwrap_or_default();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(self.input.chars().count()),
                        )));
                    state.store(ui.ctx(), composer_id);
                }
                if !self.attachments.is_empty() {
                    let mut remove = None;
                    ui.horizontal_wrapped(|ui| {
                        for (index, (name, _, bytes)) in self.attachments.iter().enumerate() {
                            egui::Frame::new()
                                .fill(egui::Color32::from_gray(48))
                                .corner_radius(6)
                                .inner_margin(6)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(icon_text(lucide_icons::Icon::File, 14.0));
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(name).small())
                                                .truncate(),
                                        )
                                        .on_hover_text(
                                            format!("{name} · {} KB", bytes.len() / 1000),
                                        );
                                        if ui
                                            .add(
                                                egui::Button::new(icon_text(
                                                    lucide_icons::Icon::X,
                                                    12.0,
                                                ))
                                                .frame(false),
                                            )
                                            .on_hover_text("Remove file")
                                            .clicked()
                                        {
                                            remove = Some(index);
                                        }
                                    });
                                });
                        }
                    });
                    if let Some(index) = remove {
                        self.attachments.remove(index);
                    }
                    ui.add_space(5.0);
                }
                let draft_scroll = egui::ScrollArea::vertical()
                    .id_salt("draft_scroll")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.input)
                                .text_color(if cfg!(target_os = "android") {
                                    egui::Color32::TRANSPARENT
                                } else {
                                    ui.visuals().text_color()
                                })
                                .id(composer_id)
                                .hint_text(if cfg!(target_os = "android") {
                                    ""
                                } else if self.task_chat {
                                    "What should I do, and when? e.g. Every morning at 9…"
                                } else {
                                    "Message…  / for commands"
                                })
                                .frame(egui::Frame::NONE)
                                .event_filter(egui::EventFilter {
                                    tab: !candidates.is_empty(),
                                    escape: !candidates.is_empty(),
                                    horizontal_arrows: true,
                                    vertical_arrows: true,
                                    ..Default::default()
                                })
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    egui::Key::Enter,
                                )),
                        )
                    });
                let response = draft_scroll.inner;
                #[cfg(target_os = "android")]
                android_text::show(
                    &format!("{}|{}", self.url, self.session),
                    &self.input,
                    response.rect.intersect(draft_scroll.inner_rect),
                    ui.ctx().pixels_per_point(),
                    self.focus_composer,
                );
                if response.changed() {
                    self.slash_dismissed = false;
                    self.slash_selected = 0;
                }
                if self.focus_composer {
                    #[cfg(not(target_os = "android"))]
                    response.request_focus();
                    self.focus_composer = false;
                }
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().interact_size.y = 36.0;
                    if ui
                        .add_enabled(
                            self.upload.is_none(),
                            egui::Button::new(icon_text(lucide_icons::Icon::Plus, 18.0))
                                .frame(false),
                        )
                        .on_hover_text("Upload files · 6 MB per file, 12 MB total")
                        .clicked()
                    {
                        self.composer_error.clear();
                        self.upload = Some((self.session.clone(), uploads::pick(ui.ctx().clone())));
                    }
                    let old = self.model.clone();
                    let short = self
                        .model
                        .rsplit('/')
                        .next()
                        .unwrap_or(&self.model)
                        .to_string();
                    egui::ComboBox::from_id_salt("model")
                        .selected_text(if short.is_empty() { "Model".into() } else { short })
                        .width(if ui.available_width() < 280.0 {
                            104.0
                        } else {
                            130.0
                        })
                        .show_ui(ui, |ui| {
                            for model in &self.models {
                                ui.selectable_value(&mut self.model, model.clone(), model);
                            }
                        })
                        .response
                        .on_hover_text("Model");
                    if self.model != old {
                        self.remember_selection();
                        self.patch_selection();
                    }
                    let old = self.thinking.clone();
                    egui::ComboBox::from_id_salt("thinking")
                        .selected_text(&self.thinking)
                        .width(82.0)
                        .show_ui(ui, |ui| {
                            for level in ["low", "medium", "high", "xhigh", "max"] {
                                ui.selectable_value(&mut self.thinking, level.into(), level);
                            }
                        })
                        .response
                        .on_hover_text("Reasoning effort");
                    if self.thinking != old {
                        self.remember_selection();
                        self.patch_selection();
                    }
                    ui.horizontal(|ui| {
                        if self.running {
                            if ui
                                .add(
                                    egui::Button::new(icon_text(lucide_icons::Icon::Square, 17.0))
                                        .min_size(egui::vec2(32.0, 32.0)),
                                )
                                .on_hover_text("Stop response")
                                .clicked()
                            {
                                self.rpc("chat.abort", json!({"sessionKey":self.session}));
                            }
                        } else if ui
                            .add_enabled(
                                self.connected
                                    && !self.history_loading
                                    && self.upload.is_none()
                                    && (!self.input.trim().is_empty()
                                        || !self.attachments.is_empty()),
                                egui::Button::new(icon_text(lucide_icons::Icon::ArrowUp, 19.0))
                                    .min_size(egui::vec2(32.0, 32.0)),
                            )
                            .on_hover_text("Send · Enter")
                            .clicked()
                        {
                            send = true;
                        }
                        if self.upload.is_some() {
                            ui.spinner();
                        }
                    });
                });
                if !self.composer_error.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.composer_error);
                }
            });
        if send && accept.is_none() {
            self.send(ui.ctx());
            self.focus_composer = true;
        }
    }
    fn restore_unsent(&mut self, params: &Value) {
        let Some(key) = params["sessionKey"].as_str() else {
            return;
        };
        if self.deleted_sessions.contains(key) {
            return;
        }
        let text = params["message"].as_str().unwrap_or("");
        let files: Vec<_> = params["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some((
                    a["fileName"].as_str()?.to_owned(),
                    a["mimeType"].as_str()?.to_owned(),
                    base64::engine::general_purpose::STANDARD
                        .decode(a["content"].as_str()?)
                        .ok()?,
                ))
            })
            .collect();
        let (input, attachments) = if self.session == key {
            self.running = false;
            let identity = params["idempotencyKey"]
                .as_str()
                .map(|id| format!("local:{id}"));
            if let Some(index) = self.messages.iter().rposition(|m| {
                m.role == "user"
                    && (identity
                        .as_ref()
                        .is_some_and(|id| m.item_id.as_ref() == Some(id))
                        || (m.item_id.is_none() && m.text == text))
            }) {
                self.messages.remove(index);
            }
            (&mut self.input, &mut self.attachments)
        } else {
            let draft = self.drafts.entry(key.into()).or_default();
            (&mut draft.input, &mut draft.attachments)
        };
        if input.is_empty() {
            *input = text.into();
        } else if input != text {
            *input = format!("{text}\n\n{input}");
        }
        attachments.extend(files);
        self.active_sessions.remove(key);
        self.live.remove(key);
    }
    fn forget_deleted_session(&mut self, key: &str) {
        if !self.deleted_sessions.insert(key.to_owned()) {
            return;
        }
        if self.delete_chat.as_ref().is_some_and(|(k, _)| k == key) {
            self.delete_chat = None;
        }
        self.draft_queue.forget(key);
        let rev = self
            .desktops
            .workspace
            .get("draft", key)
            .and_then(|v| v["rev"].as_u64())
            .unwrap_or(0);
        self.desktops.workspace.rpc(
            "workspace.put",
            json!({"kind":"draft","id":key,"expectedRevision":rev,"value":{"text":""}}),
        );
        self.sessions.retain(|(k, _)| k != key);
        self.session_settings.remove(key);
        self.drafts.remove(key);
        self.fallback_titles.remove(key);
        self.title_requests.retain(|_, k| k != key);
        self.selection_pending.retain(|_, k| k != key);
        self.pending_sends
            .retain(|_, p| p["sessionKey"].as_str() != Some(key));
        self.selection_edited.remove(key);
        self.active_sessions.remove(key);
        self.live.remove(key);
        if self.session == key {
            if let Some((next, _)) = self.sessions.first().cloned() {
                self.select_session_impl(next, true, false);
            } else {
                self.select_session_impl(
                    format!("agent:main:d4c-{}", uuid::Uuid::new_v4()),
                    false,
                    false,
                );
            }
        }
    }
    fn patch_selection(&mut self) {
        self.selection_edited.insert(self.session.clone());
        self.session_settings.insert(
            self.session.clone(),
            (self.model.clone(), self.thinking.clone()),
        );
        // Model preferences for an empty composer stay local until its first send.
        // sessions.patch materializes a server session, even without messages.
        if self.deleted_sessions.contains(&self.session)
            || (!self.sessions.iter().any(|(key, _)| key == &self.session)
                && self.messages.is_empty())
        {
            return;
        }
        let mut patch = json!({"key":self.session,"thinkingLevel":self.thinking});
        if !self.model.is_empty() { patch["model"] = json!(self.model); }
        if let Some(id) = self.rpc("sessions.patch", patch) {
            self.selection_pending.insert(id, self.session.clone());
        }
    }
    fn remember_selection(&mut self) {
        self.preferences.model_defaults.insert(
            self.url.clone(),
            (self.model.clone(), self.thinking.clone()),
        );
        if let Err(e) = self.preferences.save() {
            self.composer_error = format!("Could not save model preference: {e}");
        }
        self.defaults_dirty = true;
    }

    fn send(&mut self, ctx: &egui::Context) {
        if !self.connected
            || self.running
            || self.history_loading
            || self.upload.is_some()
            || (self.input.trim().is_empty() && self.attachments.is_empty())
        {
            return;
        }
        let text = std::mem::take(&mut self.input);
        if self.desktops.slash(&text, &self.session, &self.url, ctx) {
            return;
        }
        // Ignore any background tail snapshot requested before this send.
        self.history_request = None;
        self.history_dirty = true;
        let send_identity = uuid::Uuid::new_v4().to_string();
        self.messages.push(ChatMessage {
            media: self
                .attachments
                .iter()
                .map(|(n, m, b)| media::Attachment::new(n.clone(), m.clone(), b.clone()))
                .collect(),
            role: "user".into(),
            text: text.clone(),
            item_id: Some(format!("local:{send_identity}")),
        });
        let attachments: Vec<_> = self.attachments.drain(..).map(|(name, mime, bytes)| {
            let kind = if mime.starts_with("image/") { "image" } else { "file" };
            json!({"type":kind,"fileName":name,"mimeType":mime,"content":base64::engine::general_purpose::STANDARD.encode(bytes)})
        }).collect();
        if !self.sessions.iter().any(|(key, _)| key == &self.session) {
            self.patch_selection();
        }
        let params = json!({"sessionKey":self.session,"message":text,"attachments":attachments,"idempotencyKey":send_identity});
        if let Some(id) = self
            .selection_pending
            .iter()
            .find(|(_, key)| *key == &self.session)
            .map(|(id, _)| id.clone())
        {
            eprintln!("D4C waiting for model id={id}");
            self.pending_sends.insert(id, params);
        } else {
            self.rpc("chat.send", params);
        }
        self.running = true;
        self.active_sessions.insert(self.session.clone());
        self.live
            .insert(self.session.clone(), activity::Live::default());
        if !self.sessions.iter().any(|(key, _)| key == &self.session) {
            let title: String = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(60)
                .collect();
            let title = if title.is_empty() {
                "Attached files".into()
            } else {
                title
            };
            self.fallback_titles
                .insert(self.session.clone(), title.clone());
            self.sessions.insert(0, (self.session.clone(), title));
        }
        self.slash_dismissed = false;
        self.slash_selected = 0;
    }
}
fn message_text(v: &Value) -> String {
    if let Some(s) = v["content"].as_str() {
        return s.into();
    }
    v["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
impl eframe::App for D4c {
    #[cfg(target_os = "android")]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.save_server_drafts();
        self.servers.select(&self.url, &self.session);
        self.servers.save();
    }
    #[cfg(not(target_os = "android"))]
    fn on_exit(&mut self) {
        let _ = self.save_server_drafts();
        self.servers.select(&self.url, &self.session);
        self.servers.save();
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        {
            let visible = platform::content_rect(ui.ctx());
            // Use a child UI: shrinking just the clip rectangle would leave the
            // bottom panel positioned under the keyboard.
            ui.scope_builder(egui::UiBuilder::new().max_rect(visible), |ui| {
                ui.set_clip_rect(visible);
                self.render_ui(ui);
            });
            android_text::flush_clipboard(ui.ctx());
            // NativeActivity's content rectangle can change without a resize
            // event; refresh while the keyboard animates even if no keys arrive.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        #[cfg(not(target_os = "android"))]
        self.render_ui(ui);
    }
}
impl D4c {
    fn load_media(&mut self) {
        if !self.connected {
            return;
        }
        let mut requests = vec![];
        for (i, message) in self.messages.iter_mut().enumerate() {
            for (j, media) in message.media.iter_mut().enumerate() {
                if let Some(token) = self.media_auth.as_ref() {
                    media.configure_http(&self.url, token, &self.session);
                }
                if self.media_requests.len() + requests.len() >= 4 {
                    break;
                }
                if media.size == 0 && !media.requested {
                    if let Some(id) = media.artifact.as_ref() {
                        media.requested = true;
                        requests.push((i, j, id.clone()));
                    }
                }
            }
        }
        for (i, j, artifact) in requests {
            if let Some(id) = self.rpc(
                "artifacts.download",
                json!({"sessionKey":self.session,"artifactId":artifact,"previewWidth":640}),
            ) {
                self.media_requests.insert(id, (self.session.clone(), i, j));
            }
        }
    }
    fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::selectable(
                        !self.show_cron,
                        button_label(lucide_icons::Icon::Monitor, "Desktop"),
                    )
                    .min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                self.show_cron = false;
            }
            if ui
                .add(
                    egui::Button::selectable(
                        self.show_cron,
                        button_label(lucide_icons::Icon::CalendarClock, "Tasks"),
                    )
                    .min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                self.show_cron = true;
                self.cron.refresh();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(icon_text(lucide_icons::Icon::X, 18.0)).frame(false))
                    .on_hover_text("Close workspace")
                    .clicked()
                {
                    self.workspace_open = false;
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(10.0);
        egui::ScrollArea::vertical()
            .id_salt(("workspace_content", &self.session, self.show_cron))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.show_cron {
                    self.cron.session_ui(
                        ui,
                        &self.session,
                        self.connected,
                        &self.models,
                        &self.model,
                        &self.thinking,
                    );
                } else {
                    self.desktops.ui(ui, &self.url, &self.session);
                }
            });
    }
    fn render_ui(&mut self, ui: &mut egui::Ui) {
        #[cfg(target_os = "android")]
        if let Some((session, text, revision)) = android_text::begin(ui.ctx()) {
            if session == format!("{}|{}", self.url, self.session) {
                android_text::acknowledge(revision);
                self.input = text;
                self.slash_dismissed = false;
                self.slash_selected = 0;
            }
        }
        self.events(ui.ctx());
        self.servers.update_active(
            &self.url,
            &self.sessions,
            &self.active_sessions,
            self.connected,
            &self.status,
        );
        self.servers
            .poll(&self.preferences.connections, &self.url, ui.ctx());
        self.reconcile_history();
        self.desktops.poll_ui(ui.ctx());
        self.sync_workspace();
        self.desktops
            .follow_session(&self.session, &self.url, ui.ctx());
        if self.desktops.open {
            self.workspace_open = true;
            self.desktops.open = false;
        }
        self.poll_upload();
        self.load_media();
        #[cfg(target_os = "android")]
        if let Some(session) = android_notifications::take_session() {
            self.select_session(session, true);
        }
        self.notifier.poll();
        self.cron.background(self.connected);
        self.center.bind(self.cron.jobs());
        let mut alert = None;
        for run in self.cron.take_finished() {
            if let Some(entry) = self.center.cron(&run) {
                alert = Some(entry);
            }
        }
        if self.connected {
            for run in self.center.pending_results() {
                self.cron.load_result(&run);
            }
        }
        if let Some(entry) = alert {
            self.notifier.send(
                entry["title"].as_str().unwrap_or("Scheduled task"),
                entry["body"].as_str().unwrap_or(""),
                self.preferences.desktop,
                self.preferences.sound,
            );
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        let compact = ui.ctx().content_rect().width() < 640.0;
        if compact && !self.compact_previous {
            self.sidebar_open = false;
        }
        self.compact_previous = compact;
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() && self.upload.is_none() {
            self.upload = Some((
                self.session.clone(),
                uploads::load(
                    dropped.iter().map(|f| f.path().to_path_buf()).collect(),
                    ui.ctx().clone(),
                ),
            ));
        }
        egui::Panel::top("top_bar")
            .exact_size(52.0)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(29, 30, 34))
                    .inner_margin(egui::Margin::symmetric(18, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let icon = if self.sidebar_open {
                        lucide_icons::Icon::PanelLeftClose
                    } else {
                        lucide_icons::Icon::PanelLeftOpen
                    };
                    if ui
                        .add(egui::Button::new(icon_text(icon, 20.0)).frame(false))
                        .on_hover_text("Toggle sidebar")
                        .clicked()
                    {
                        self.sidebar_open = !self.sidebar_open;
                        if compact {
                            self.workspace_open = false;
                            self.notifications_open = false;
                        }
                    }
                    let status_width = 112.0;
                    let (title_rect, title_response) = ui.allocate_exact_size(
                        egui::vec2((ui.available_width() - status_width).max(24.0), 36.0),
                        egui::Sense::hover(),
                    );
                    paint_ellipsis(
                        ui,
                        title_rect,
                        if self.settings_open {
                            "Settings"
                        } else {
                            self.chat_title()
                        },
                        15.0,
                        ui.visuals().text_color(),
                    );
                    title_response.on_hover_text(self.chat_title());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(icon_text(lucide_icons::Icon::Layers, 20.0))
                                    .frame(self.workspace_open)
                                    .min_size(egui::vec2(40.0, 36.0)),
                            )
                            .on_hover_text("Chat workspace")
                            .clicked()
                        {
                            self.workspace_open = !self.workspace_open;
                            self.notifications_open = false;
                            self.settings_open = false;
                            if compact {
                                self.sidebar_open = false;
                            }
                            ui.ctx().memory_mut(|m| {
                                if let Some(id) = m.focused() {
                                    m.surrender_focus(id);
                                }
                            });
                        }
                        let count = self.center.unread(None);
                        let text = if count > 0 {
                            button_label(
                                lucide_icons::Icon::Bell,
                                &if count > 99 {
                                    "99+".into()
                                } else {
                                    count.to_string()
                                },
                            )
                        } else {
                            button_label(lucide_icons::Icon::Bell, "")
                        };
                        if ui
                            .add(
                                egui::Button::new(text)
                                    .min_size(egui::vec2(44.0, 36.0))
                                    .frame(self.notifications_open),
                            )
                            .on_hover_text("Notifications")
                            .clicked()
                        {
                            self.notifications_open = !self.notifications_open;
                            self.workspace_open = false;
                            ui.ctx().memory_mut(|m| {
                                if let Some(id) = m.focused() {
                                    m.surrender_focus(id);
                                }
                            });
                            if compact {
                                self.sidebar_open = false;
                            }
                        }
                    });
                });
            });
        let sidebar_width = if compact {
            (ui.available_width() - 1.0).max(1.0)
        } else {
            (ui.available_width() * 0.30).clamp(240.0, 420.0)
        };
        let mut sidebar_open = self.sidebar_open;
        egui::Panel::left("sidebar")
            .exact_size(sidebar_width)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(25, 26, 30))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 49, 54)))
                    .inner_margin(egui::Margin::same(16)),
            )
            .show_collapsible(ui, &mut sidebar_open, |ui| {
                ui.horizontal(|ui| {
                    ui.label(icon_text(lucide_icons::Icon::Search, 16.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session_search)
                            .id(egui::Id::new("session_search"))
                            .hint_text("Search chats")
                            .desired_width((ui.available_width() - 32.0).max(60.0)),
                    );
                    if !self.session_search.is_empty()
                        && ui
                            .add(
                                egui::Button::new(icon_text(lucide_icons::Icon::X, 15.0))
                                    .frame(false),
                            )
                            .on_hover_text("Clear search")
                            .clicked()
                    {
                        self.session_search.clear();
                    }
                });
                ui.add_space(6.0);
                let query = self.session_search.trim().to_lowercase();
                let mut matches = 0;
                let list_height = (ui.available_height() - 56.0).max(20.0);
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .min_scrolled_height(0.0)
                    .id_salt("chats")
                    .show(ui, |ui| {
                        let mut selected: Option<(usize, Option<String>)> = None;
                        let mut rename = None;
                        let mut delete = None;
                        let connections = self.preferences.connections.clone();
                        for (server_index, connection) in connections.iter().enumerate() {
                            let current = connection.url == self.url;
                            let Some(row) = self.servers.rows.get(&connection.url) else {
                                continue;
                            };
                            let online = row.online;
                            let expanded = row.expanded || !self.session_search.is_empty();
                            let status = row.status.clone();
                            let sessions = row.sessions.clone();
                            let matching = sessions
                                .iter()
                                .filter(|s| {
                                    self.session_search.is_empty()
                                        || s.title.to_lowercase().contains(&query)
                                })
                                .count();
                            if !query.is_empty()
                                && matching == 0
                                && !connection.name.to_lowercase().contains(&query)
                            {
                                continue;
                            }
                            let (toggle, new_chat) = ui
                                .push_id(&connection.url, |ui| {
                                    server_row(
                                        ui,
                                        &connection.name,
                                        online,
                                        current,
                                        expanded,
                                        &status,
                                    )
                                })
                                .inner;
                            if new_chat {
                                selected = Some((server_index, None));
                            }
                            if toggle {
                                self.servers.toggle(&connection.url);
                            }
                            if expanded {
                                ui.indent(("server_chats", &connection.url), |ui| {
                                    for session in &sessions {
                                        if !query.is_empty()
                                            && !session.title.to_lowercase().contains(&query)
                                            && !connection.name.to_lowercase().contains(&query)
                                        {
                                            continue;
                                        }
                                        matches += 1;
                                        ui.push_id((&connection.url, &session.key), |ui| {
                                            if current {
                                                let count = self.center.unread(Some(&session.key))
                                                    + self.questions.pending_count(&session.key);
                                                let (click, edit, remove) = chat_row(
                                                    ui,
                                                    &session.key,
                                                    &session.title,
                                                    self.session == session.key,
                                                    count,
                                                    session.running,
                                                    true,
                                                );
                                                if click {
                                                    selected = Some((
                                                        server_index,
                                                        Some(session.key.clone()),
                                                    ));
                                                }
                                                if edit {
                                                    rename = Some((
                                                        session.key.clone(),
                                                        session.title.clone(),
                                                    ));
                                                }
                                                if remove {
                                                    delete = Some((
                                                        session.key.clone(),
                                                        session.title.clone(),
                                                    ));
                                                }
                                            } else {
                                                let (click, _, _) = chat_row(
                                                    ui,
                                                    &session.key,
                                                    &session.title,
                                                    false,
                                                    0,
                                                    session.running,
                                                    false,
                                                );
                                                if click {
                                                    selected = Some((
                                                        server_index,
                                                        Some(session.key.clone()),
                                                    ));
                                                }
                                            }
                                        });
                                        ui.add_space(2.0);
                                    }
                                    if sessions.is_empty() {
                                        ui.label(
                                            egui::RichText::new(if online {
                                                "No chats"
                                            } else {
                                                "Offline"
                                            })
                                            .small()
                                            .weak(),
                                        );
                                    }
                                });
                            }
                            ui.add_space(8.0);
                        }
                        if let Some((index, key)) = selected {
                            let load = key.is_some();
                            let target = key.unwrap_or_else(|| {
                                format!("agent:main:d4c-{}", uuid::Uuid::new_v4())
                            });
                            if index == self.preferences.selected {
                                self.select_session(target, load);
                            } else {
                                self.switch_connection(index, ui.ctx());
                                if self.preferences.selected == index {
                                    if self.session != target {
                                        self.drafts.insert(
                                            self.session.clone(),
                                            Draft {
                                                input: std::mem::take(&mut self.input),
                                                attachments: std::mem::take(&mut self.attachments),
                                            },
                                        );
                                        self.session = target;
                                        let draft =
                                            self.drafts.remove(&self.session).unwrap_or_default();
                                        self.input = draft.input;
                                        self.attachments = draft.attachments;
                                    }
                                    self.explicit_selection = true;
                                    self.servers.select(&self.url, &self.session);
                                    self.settings_open = false;
                                }
                            }
                            if compact {
                                self.sidebar_open = false;
                            }
                        }
                        if let Some(value) = rename {
                            self.rename = Some(value);
                            self.rename_error.clear();
                        }
                        if let Some(value) = delete {
                            if self.delete_pending.is_none() {
                                self.delete_chat = Some(value);
                                self.delete_error.clear();
                            }
                        }
                        if matches == 0 && !query.is_empty() {
                            ui.weak("No matching chats");
                        }
                    });
                ui.add_space((ui.available_height() - 44.0).max(0.0));
                ui.separator();
                if navigation_footer(ui).clicked() {
                    self.settings_open = true;
                    self.notifications_open = false;
                    self.workspace_open = false;
                    if compact {
                        self.sidebar_open = false;
                    }
                }
            });
        if self.sidebar_open != sidebar_open && !compact {
            self.sidebar_open = sidebar_open;
        }

        if let Some(index) = self.switch_pending {
            let mut accept = false;
            let mut cancel = false;
            egui::Window::new("Switch server?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,[0.0,0.0]).default_width(280.0).show(ui.ctx(),|ui|{
                ui.label("Work is still running on this server. It will continue there while you switch.");
                ui.horizontal(|ui|{if ui.button("Stay here").clicked(){cancel=true;}if ui.button("Switch server").clicked(){accept=true;}});
            });
            if cancel {
                self.switch_pending = None;
            }
            if accept {
                self.switch_pending = None;
                self.switch_connection(index, ui.ctx());
            }
        }
        if let Some((key, title)) = self.delete_chat.clone() {
            let mut cancel = false;
            let mut confirm = false;
            egui::Window::new("Delete chat?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width((ui.ctx().content_rect().width() - 48.0).clamp(180.0, 340.0))
                .show(ui.ctx(), |ui| {
                    ui.label(&title);
                    ui.label("This removes the chat and its transcript.");
                    if !self.delete_error.is_empty() {
                        ui.colored_label(egui::Color32::LIGHT_RED, &self.delete_error);
                    }
                    ui.horizontal(|ui| {
                        cancel = ui
                            .add_enabled(self.delete_pending.is_none(), egui::Button::new("Cancel"))
                            .clicked();
                        confirm = ui
                            .add_enabled(
                                self.connected
                                    && self.delete_pending.is_none()
                                    && !self.active_sessions.contains(&key),
                                egui::Button::new(if self.delete_pending.is_some() {
                                    "Deleting…"
                                } else {
                                    "Delete"
                                }),
                            )
                            .clicked();
                    });
                });
            if cancel {
                self.delete_chat = None;
            } else if confirm {
                self.delete_error.clear();
                self.delete_pending = self
                    .rpc(
                        "sessions.delete",
                        json!({"key":key,"deleteTranscript":true}),
                    )
                    .map(|id| (id, key));
            }
        }
        if let Some((key, mut name)) = self.rename.clone() {
            let mut cancel = false;
            let mut save = false;
            egui::Window::new("Rename chat")
                .max_width((ui.ctx().content_rect().width() - 48.0).max(180.0))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label("Chat name");
                    let input = ui.add(egui::TextEdit::singleline(&mut name).desired_width(300.0));
                    if !self.rename_error.is_empty() {
                        ui.colored_label(egui::Color32::LIGHT_RED, &self.rename_error);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        cancel = ui
                            .add_enabled(self.rename_pending.is_none(), egui::Button::new("Cancel"))
                            .clicked();
                        save = ui
                            .add_enabled(
                                self.rename_pending.is_none() && !name.trim().is_empty(),
                                egui::Button::new("Save"),
                            )
                            .clicked()
                            || (input.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                && !name.trim().is_empty()
                                && self.rename_pending.is_none());
                    });
                });
            if cancel {
                self.rename = None;
            } else {
                self.rename = Some((key.clone(), name.clone()));
                if save {
                    let req =
                        gateway::rpc("sessions.patch", json!({"key":key,"label":name.trim()}));
                    if let Some(g) = &self.gateway {
                        let id = req["id"].as_str().unwrap_or("").to_string();
                        if g.tx.send(req).is_ok() {
                            self.rename_pending = Some(id);
                            self.rename_error.clear();
                        } else {
                            self.rename_error = "Gateway disconnected".into();
                        }
                    } else {
                        self.rename_error = "Connect to rename chats".into();
                    }
                }
            }
        }
        if self.workspace_open && !compact && !self.settings_open && !self.notifications_open {
            egui::Panel::right("chat_workspace")
                .default_size(360.0_f32.min(ui.available_width() * 0.45))
                .min_size(260.0_f32.min(ui.available_width() * 0.45))
                .max_size((ui.available_width() * 0.5).max(260.0))
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(25, 27, 31))
                        .inner_margin(14),
                )
                .show(ui, |ui| self.workspace_ui(ui));
        }
        if self.notifications_open && !compact {
            egui::Panel::right("notification_center")
                .exact_size(340.0_f32.min(ui.available_width() * 0.45))
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(25, 27, 31))
                        .inner_margin(16),
                )
                .show(ui, |ui| {
                    self.notification_ui(ui);
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(19, 20, 23))
                    .inner_margin(egui::Margin::same(if compact { 12 } else { 20 })),
            )
            .show(ui, |ui| {
                if compact && self.sidebar_open {
                    return;
                }
                if self.notifications_open && compact {
                    self.notification_ui(ui);
                } else if self.workspace_open && compact {
                    self.workspace_ui(ui);
                } else if self.settings_open {
                    self.settings_ui(ui);
                } else if !self.connected {
                    ui.heading("Connect to OpenClaw");
                    ui.add_space(8.0);
                    ui.label("Gateway URL");
                    ui.text_edit_singleline(&mut self.url);
                    ui.add_space(8.0);
                    ui.label("Gateway token");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.token)
                            .password(true)
                            .hint_text("Paste your Gateway token"),
                    );
                    ui.add_space(10.0);
                    if ui.button("Connect").clicked() {
                        if preferences::valid_url(&self.url) {
                            if self.preferences.current().url != self.url {
                                let index = if let Some(index) = self
                                    .preferences
                                    .connections
                                    .iter()
                                    .position(|c| c.url == self.url)
                                {
                                    index
                                } else {
                                    self.preferences.connections.push(preferences::Connection {
                                        name: "OpenClaw".into(),
                                        url: self.url.clone(),
                                    });
                                    self.preferences.connections.len() - 1
                                };
                                let token = std::mem::take(&mut self.token);
                                self.switch_with_token(index, ui.ctx(), token);
                            } else {
                                self.reconnect(ui.ctx());
                            }
                        } else {
                            self.status =
                                "Use a secure wss:// URL without credentials or query tokens"
                                    .into();
                        }
                    }
                    ui.add_space(8.0);
                    if self.gateway.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Reconnecting automatically");
                        });
                    }
                    if self.status != "Not connected" {
                        ui.label(&self.status);
                    }
                } else {
                    if ui.input(|i| i.viewport().focused.unwrap_or(false)) && !self.history_loading
                    {
                        self.center.read(Some(&self.session));
                    }
                    egui::Panel::bottom("composer_panel")
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 10)))
                        .show(ui, |ui| {
                            self.composer(ui);
                        });
                    let mut load_older = false;
                    let mut message_scroll = egui::ScrollArea::vertical()
                        .id_salt(("messages", &self.session))
                        .auto_shrink([false, false])
                        .scroll_source(egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Always, ..Default::default() })
                        .stick_to_bottom(self.prepend_anchor.is_none() && self.restore_scroll.is_none());
                    if let Some(offset) = self.restore_scroll.take() { message_scroll = message_scroll.vertical_scroll_offset(offset); }
                    let scroll = message_scroll.show(ui, |ui| {
                            if self.older_cursor.is_some() {
                                if self.older_request.is_some() { ui.spinner(); }
                                else if ui.small_button("Earlier messages").clicked() { load_older = true; }
                            }
                            if !self.older_error.is_empty() { ui.colored_label(egui::Color32::LIGHT_RED, &self.older_error); }
                            if self.history_loading {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Loading conversation…");
                                });
                            }
                            if self.messages.is_empty()
                                && !self.history_loading
                                && !self.questions.has_session(&self.session)
                            {
                                ui.add_space((ui.available_height() * 0.25).max(24.0));
                                ui.vertical_centered(|ui| {
                                    ui.heading(if self.task_chat {
                                        "What should I schedule?"
                                    } else {
                                        "What are we working on?"
                                    });
                                });
                            }
                            let mut index = 0;
                            while index < self.messages.len() {
                                let message = &self.messages[index];
                                if is_tool_message(message) {
                                    let start = index;
                                    while index < self.messages.len() && is_tool_message(&self.messages[index]) { index += 1; }
                                    egui::CollapsingHeader::new(format!("Activity · {} tool calls", index - start))
                                        .id_salt(("tool_group", &self.session, message.item_id.as_deref().unwrap_or("tool")))
                                        .show(ui, |ui| {
                                            for (offset, message) in self.messages[start..index].iter().enumerate() {
                                                ui.push_id((start, offset), |ui| message_bubble(ui, message, &mut self.markdown_cache));
                                                ui.add_space(4.0);
                                            }
                                        });
                                } else {
                                    ui.push_id((&self.session, index), |ui| message_bubble(ui, message, &mut self.markdown_cache));
                                    index += 1;
                                }
                                ui.add_space(12.0);
                            }
                            // Scheduled work stays in Tasks and owner-linked notifications.
                            for request in self.questions.ui(ui, &self.session, self.connected) {
                                if let Some(g) = &self.gateway {
                                    let _ = g.tx.send(request);
                                }
                            }
                            if self.running {
                                self.live.entry(self.session.clone()).or_default().ui(ui);
                            }
                            if self.jump_latest { ui.scroll_to_cursor(Some(egui::Align::BOTTOM)); }
                        });
                    self.jump_latest = false;
                    let remaining = scroll.content_size.y - scroll.inner_rect.height() - scroll.state.clamped_offset().y;
                    if remaining > 80.0 {
                        egui::Area::new(egui::Id::new("jump_latest"))
                            .order(egui::Order::Foreground)
                            .pivot(egui::Align2::CENTER_BOTTOM)
                            .fixed_pos(scroll.inner_rect.center_bottom() - egui::vec2(0.0, 12.0))
                            .show(ui.ctx(), |ui| {
                                let (rect, response) = ui.allocate_exact_size(egui::vec2(42.0,42.0), egui::Sense::click());
                                let center = rect.center();
                                ui.painter().circle_filled(center,21.0,egui::Color32::from_rgb(55,60,68));
                                let stroke = egui::Stroke::new(2.0,egui::Color32::from_gray(235));
                                // Lucide ArrowDown geometry, centered independently of font metrics.
                                ui.painter().line_segment([center + egui::vec2(0.0,-7.0),center + egui::vec2(0.0,7.0)],stroke);
                                ui.painter().add(egui::Shape::line(vec![center + egui::vec2(-7.0,0.0),center + egui::vec2(0.0,7.0),center + egui::vec2(7.0,0.0)],stroke));
                                if response.on_hover_text("Latest messages").clicked() {
                                    self.jump_latest = true; ui.ctx().request_repaint();
                                }
                            });
                    }
                    if let Some((old_height, old_offset)) = self.prepend_anchor.take() {
                        self.restore_scroll = Some((old_offset + scroll.content_size.y - old_height).max(0.0));
                        ui.ctx().request_repaint();
                    }
                    self.history_scroll = (scroll.content_size.y, scroll.state.clamped_offset().y);
                    if (load_older || (scroll.state.clamped_offset().y < 50.0 && self.older_error.is_empty() && self.restore_scroll.is_none())) && self.older_request.is_none() && self.history_request.is_none() {
                        if let Some(cursor) = self.older_cursor.clone() {
                            if let Some(id) = self.rpc("chat.history", json!({"sessionKey":self.session,"limit":100,"cursor":cursor,"toolResultMaxChars":2000})) {
                                self.older_error.clear();
                                self.older_request = Some((id, self.session.clone()));
                            }
                        }
                    }
                }
            });
        if let Some(request) = self.cron.take_chat_request() {
            let load = request.session.is_some();
            let key = request
                .session
                .unwrap_or_else(|| format!("agent:main:d4c-{}", uuid::Uuid::new_v4()));
            self.select_session(key, load);
            if !request.draft.is_empty() {
                if !self.input.is_empty() {
                    self.input.push_str("\n\n");
                }
                self.input.push_str(&request.draft);
            }
            let id = egui::Id::unique("message_input");
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(self.input.chars().count()),
                )));
            state.store(ui.ctx(), id);
            self.task_chat = true;
            self.workspace_open = false;
            self.focus_composer = true;
        }
        if self.cron.open_history {
            self.cron.open_history = false;
            self.show_cron = true;
            self.workspace_open = true;
            self.settings_open = false;
            if compact {
                self.sidebar_open = false;
            }
        }
        self.desktops.viewer_ui(ui.ctx());
        self.cron.flush(self.gateway.as_ref());
        #[cfg(target_os = "android")]
        android_text::finish();
    }
}
fn server_draft_file(url: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("server-drafts-{:x}.json", Sha256::digest(url.as_bytes()))
}
fn is_tool_message(message: &ChatMessage) -> bool {
    message
        .item_id
        .as_deref()
        .is_some_and(|id| id.starts_with("tool:"))
}
fn same_chat_message(a: &ChatMessage, b: &ChatMessage) -> bool {
    match (&a.item_id, &b.item_id) {
        (Some(a), Some(b)) => a == b,
        // Legacy history/live echoes may not carry an ID. Match only nonempty
        // content so distinct empty image/tool messages are never coalesced.
        _ => a.role == b.role && !a.text.is_empty() && a.text == b.text,
    }
}
// Keep bubble alignment independent of text layout direction, so code, mixed
// scripts, Markdown, and selectable text all retain normal reading order.
fn message_bubble(
    ui: &mut egui::Ui,
    message: &ChatMessage,
    cache: &mut egui_commonmark::CommonMarkCache,
) {
    let display_text = if !message.media.is_empty() {
        message
            .text
            .lines()
            .filter(|line| !line.trim_start().starts_with("MEDIA:"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        message.text.clone()
    };
    let outgoing = message.role == "user";
    let tool = message
        .item_id
        .as_deref()
        .is_some_and(|id| id.starts_with("tool:"));
    let available = ui.available_width();
    let max_width = (if outgoing { available * 0.92 } else { available }).max(1.0);
    // Bound measurement work during streaming. Code and tables get the full
    // reading width; ordinary messages shrink to their longest short line.
    let structured = !outgoing && (message.text.contains("```") || message.text.contains("|"));
    let width = if tool || structured || !message.media.is_empty() {
        max_width
    } else {
        let longest = message
            .text
            .lines()
            .take(32)
            .map(|line| line.chars().take(120).count())
            .max()
            .unwrap_or(0);
        (longest as f32 * 8.0 + 32.0)
            .max(if outgoing { 104.0 } else { 128.0 })
            .min(max_width)
    };
    ui.allocate_ui_with_layout(
        egui::vec2(available, 0.0),
        egui::Layout::left_to_right(egui::Align::TOP),
        |ui| {
            if outgoing {
                ui.add_space((available - width).max(0.0));
            }
            let fill = if outgoing {
                egui::Color32::from_rgb(28, 66, 61)
            } else {
                egui::Color32::from_rgb(33, 35, 41)
            };
            let corners = if outgoing {
                egui::CornerRadius {
                    nw: 14,
                    ne: 4,
                    sw: 14,
                    se: 14,
                }
            } else {
                egui::CornerRadius {
                    nw: 4,
                    ne: 14,
                    sw: 14,
                    se: 14,
                }
            };
            egui::Frame::new()
                .fill(fill)
                .corner_radius(corners)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width((width - 28.0).max(1.0));
                    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        if tool {
                            for media in &message.media {
                                media.ui(ui);
                                ui.add_space(6.0);
                            }
                            ui.horizontal(|ui| {
                                ui.label(
                                    icon_text(lucide_icons::Icon::Terminal, 14.0)
                                        .color(egui::Color32::from_gray(165)),
                                );
                                egui::CollapsingHeader::new(
                                    egui::RichText::new(&message.role).size(12.0),
                                )
                                .id_salt(message.item_id.as_deref().unwrap_or("tool"))
                                .show(ui, |ui| {
                                    egui::ScrollArea::both()
                                        .id_salt("tool_output")
                                        .max_height(280.0)
                                        .show(ui, |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&message.text)
                                                        .monospace()
                                                        .size(12.0),
                                                )
                                                .wrap()
                                                .selectable(true),
                                            );
                                        });
                                });
                            });
                        } else {
                            let title = match message.role.as_str() {
                                "user" => "You",
                                "assistant" => "Assistant",
                                other => other,
                            };
                            let icon = if outgoing {
                                lucide_icons::Icon::User
                            } else {
                                lucide_icons::Icon::Bot
                            };
                            ui.horizontal(|ui| {
                                let color = if outgoing {
                                    egui::Color32::from_rgb(162, 204, 191)
                                } else {
                                    egui::Color32::from_gray(167)
                                };
                                ui.label(icon_text(icon, 12.0).color(color));
                                ui.label(egui::RichText::new(title).size(11.0).color(color));
                            });
                            ui.add_space(3.0);
                            for media in &message.media {
                                media.ui(ui);
                                ui.add_space(6.0);
                            }
                            if outgoing {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&message.text)
                                            .size(14.0)
                                            .color(egui::Color32::from_gray(237)),
                                    )
                                    .wrap()
                                    .selectable(true),
                                );
                            } else {
                                ui.style_mut().text_styles.insert(
                                    egui::TextStyle::Body,
                                    egui::FontId::proportional(14.0),
                                );
                                ui.visuals_mut().override_text_color =
                                    Some(egui::Color32::from_gray(225));
                                egui_commonmark::CommonMarkViewer::new().show(
                                    ui,
                                    cache,
                                    &display_text,
                                );
                            }
                        }
                    });
                });
        },
    );
}

// egui's consume_key intentionally ignores extra Shift/Alt modifiers. Chat
// submission and autocomplete must distinguish Enter from Shift+Enter exactly.
fn consume_plain_key(input: &mut egui::InputState, key: egui::Key) -> bool {
    let mut consumed = false;
    input.events.retain(|event| {
        let matches=matches!(event,egui::Event::Key {key: event_key,pressed:true,modifiers,..} if *event_key==key && *modifiers==egui::Modifiers::NONE);
        consumed |= matches;
        !matches
    });
    consumed
}

fn paint_ellipsis(ui: &egui::Ui, rect: egui::Rect, text: &str, size: f32, color: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.into(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap.max_width = rect.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(
        rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0),
        galley,
        color,
    );
}
fn server_row(
    ui: &mut egui::Ui,
    name: &str,
    online: bool,
    active: bool,
    expanded: bool,
    status: &str,
) -> (bool, bool) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), egui::Sense::hover());
    let expand_rect =
        egui::Rect::from_min_max(egui::pos2(rect.right() - 36.0, rect.top()), rect.max);
    let add_rect = expand_rect.translate(egui::vec2(-36.0, 0.0));
    let label_rect = egui::Rect::from_min_max(rect.min, egui::pos2(add_rect.left(), rect.bottom()));
    let label = ui
        .interact(
            label_rect,
            ui.id().with(("server_label", name)),
            egui::Sense::click(),
        )
        .on_hover_text(status);
    let expand = ui
        .interact(
            expand_rect,
            ui.id().with(("server_expand", name)),
            egui::Sense::click(),
        )
        .on_hover_text(if expanded { "Collapse" } else { "Expand" });
    let add = ui
        .interact(
            add_rect,
            ui.id().with(("server_add", name)),
            egui::Sense::click(),
        )
        .on_hover_text("New chat");
    if active || label.hovered() {
        ui.painter()
            .rect_filled(rect, 8, egui::Color32::from_rgb(35, 39, 44));
    }
    let color = if online {
        egui::Color32::from_rgb(131, 212, 189)
    } else {
        egui::Color32::GRAY
    };
    ui.painter()
        .circle_filled(rect.left_center() + egui::vec2(12.0, 0.0), 3.0, color);
    paint_ellipsis(
        ui,
        egui::Rect::from_min_max(
            rect.min + egui::vec2(26.0, 0.0),
            egui::pos2(add_rect.left() - 6.0, rect.bottom()),
        ),
        name,
        14.0,
        ui.visuals().text_color(),
    );
    for (target, icon, hover) in [
        (add_rect, lucide_icons::Icon::Plus, add.hovered()),
        (
            expand_rect,
            if expanded {
                lucide_icons::Icon::ChevronDown
            } else {
                lucide_icons::Icon::ChevronRight
            },
            expand.hovered(),
        ),
    ] {
        if hover {
            ui.painter()
                .rect_filled(target.shrink(3.0), 6, egui::Color32::from_rgb(46, 48, 55));
        }
        ui.painter().text(
            target.center(),
            egui::Align2::CENTER_CENTER,
            char::from(icon),
            egui::FontId::new(17.0, egui::FontFamily::Name("lucide".into())),
            ui.visuals().text_color(),
        );
    }
    (label.clicked() || expand.clicked(), add.clicked())
}
fn navigation_footer(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, 8, egui::Color32::from_rgb(36, 38, 43));
    }
    let color = ui.visuals().text_color();
    let label = ui.painter().layout_no_wrap("Settings".into(),egui::FontId::proportional(14.0),color);
    let group_width = 18.0 + 9.0 + label.size().x;
    let left = rect.center().x - group_width / 2.0;
    ui.painter().text(egui::pos2(left + 9.0,rect.center().y),egui::Align2::CENTER_CENTER,
        char::from(lucide_icons::Icon::Settings),egui::FontId::new(18.0,egui::FontFamily::Name("lucide".into())),color);
    let y = rect.center().y - label.size().y / 2.0;
    ui.painter().galley(egui::pos2(left + 27.0,y),label,color);
    response
}
fn chat_row(
    ui: &mut egui::Ui,
    key: &str,
    title: &str,
    active: bool,
    unread: usize,
    running: bool,
    actions: bool,
) -> (bool, bool, bool) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
    let edit_rect = egui::Rect::from_center_size(
        rect.right_center() - egui::vec2(20.0, 0.0),
        egui::vec2(28.0, 28.0),
    );
    let hit_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            if actions {
                edit_rect.left() - 4.0
            } else {
                rect.right()
            },
            rect.bottom(),
        ),
    );
    let response = ui
        .interact(
            hit_rect,
            ui.scope_id().with(("chat", key)),
            egui::Sense::click(),
        )
        .on_hover_text(title);
    let hover = ui.rect_contains_pointer(rect);
    if active || hover || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            8,
            if active {
                egui::Color32::from_rgb(46, 48, 55)
            } else {
                egui::Color32::from_rgb(36, 38, 43)
            },
        );
    }
    if active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.left_center() - egui::vec2(0.0, 9.0),
                egui::vec2(3.0, 18.0),
            ),
            2,
            egui::Color32::from_rgb(178, 189, 211),
        );
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(23.0, 0.0),
        egui::Align2::CENTER_CENTER,
        char::from(lucide_icons::Icon::MessageCircle),
        egui::FontId::new(17.0, egui::FontFamily::Name("lucide".into())),
        egui::Color32::from_gray(if active { 218 } else { 145 }),
    );
    paint_ellipsis(
        ui,
        egui::Rect::from_min_max(
            rect.min + egui::vec2(43.0, 0.0),
            egui::pos2(
                (if actions {
                    edit_rect.left()
                } else {
                    rect.right()
                }) - if unread > 0 || running { 38.0 } else { 8.0 },
                rect.bottom(),
            ),
        ),
        title,
        14.0,
        egui::Color32::from_gray(if active { 240 } else { 195 }),
    );
    if unread > 0 || running {
        let badge = egui::Rect::from_center_size(
            egui::pos2(
                (if actions {
                    edit_rect.left()
                } else {
                    rect.right()
                }) - 18.0,
                rect.center().y,
            ),
            egui::vec2(26.0, 22.0),
        );
        ui.painter()
            .rect_filled(badge, 10, egui::Color32::from_rgb(45, 91, 80));
        let text = if unread > 0 {
            unread.min(99).to_string()
        } else {
            "···".into()
        };
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(207, 235, 223),
        );
    }
    let mut edit = false;
    let mut delete = false;
    if actions {
        ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
            ui.menu_button(icon_text(lucide_icons::Icon::Ellipsis, 16.0), |ui| {
                if ui.button("Rename").clicked() {
                    edit = true;
                    ui.close();
                }
                if ui
                    .add_enabled(!running, egui::Button::new("Delete…"))
                    .clicked()
                {
                    delete = true;
                    ui.close();
                }
            })
            .response
            .on_hover_text("Chat actions");
        });
    }
    (response.clicked(), edit, delete)
}
fn icon_text(icon: lucide_icons::Icon, size: f32) -> egui::RichText {
    egui::RichText::new(char::from(icon).to_string()).font(egui::FontId::new(
        size,
        egui::FontFamily::Name("lucide".into()),
    ))
}
fn button_label(icon: lucide_icons::Icon, label: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let color = egui::Color32::from_rgb(225, 225, 230);
    job.append(
        &char::from(icon).to_string(),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(17.0, egui::FontFamily::Name("lucide".into())),
            color,
            ..Default::default()
        },
    );
    job.append(
        label,
        9.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(14.0),
            color,
            ..Default::default()
        },
    );
    job
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "rs" | "py" | "js" | "ts" | "toml" | "log" | "sh" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

fn tool_title(name: &str, phase: &str, failed: bool) -> String {
    let action = match name {
        "bash" | "exec" | "shell" => "Terminal",
        "read" | "read_file" => "Read file",
        "write" | "write_file" => "Write file",
        "web_search" => "Web search",
        "browser" => "Browser",
        name if cron::is_scheduler_tool(name) => "Scheduled task",
        _ => name,
    };
    let state = if failed {
        "Failed"
    } else {
        match phase {
            "start" => "Running",
            "result" | "end" => "Done",
            _ => "Working",
        }
    };
    format!("{action}  ·  {state}")
}
fn tool_details(data: &Value) -> String {
    if cron::is_scheduler_tool(data["name"].as_str().unwrap_or("")) {
        if let Some(summary) = cron::task_summary(&data["result"]) {
            return summary;
        }
    }
    let mut lines = Vec::new();
    if let Some(args) = data["args"].as_object() {
        for (key, value) in args {
            let label = match key.as_str() {
                "command" => "Command",
                "cwd" => "Folder",
                "path" | "file_path" => "File",
                "query" => "Search",
                _ => key,
            };
            if let Some(text) = value.as_str() {
                lines.push(format!("{label}: {text}"));
            } else if value.is_number() || value.is_boolean() {
                lines.push(format!("{label}: {value}"));
            }
        }
    }
    let result = &data["result"];
    if let Some(output) = result["output"]
        .as_str()
        .or_else(|| result["text"].as_str())
        .or_else(|| result.as_str())
    {
        lines.push(format!("Output: {output}"));
    }
    for block in result["content"].as_array().into_iter().flatten().take(8) {
        if block["type"] == "text" {
            if let Some(text) = block["text"].as_str() {
                lines.push(text.chars().take(16000).collect());
            }
        }
    }
    if let Some(code) = result["exitCode"].as_i64() {
        lines.push(format!("Exit code: {code}"));
    }
    if let Some(message) = data["error"].as_str() {
        lines.push(format!("Error: {message}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod composer_tests {
    use super::*;
    fn inject(app: &mut D4c, event: gateway::Event) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (out, _sink) = std::sync::mpsc::channel();
        app.gateway = Some(gateway::test_gateway(out, rx));
        tx.send(event).unwrap();
        app.events(&egui::Context::default());
    }
    #[test]
    fn deletion_acknowledgement_uses_original_target() {
        let mut app = D4c::default();
        app.session = "other".into();
        app.sessions = vec![("a".into(), "A".into()), ("b".into(), "B".into())];
        app.delete_pending = Some(("sessions.delete:1".into(), "a".into()));
        app.delete_chat = Some(("b".into(), "B".into()));
        inject(
            &mut app,
            gateway::Event::Data(
                json!({"type":"res","id":"sessions.delete:1","ok":true,"payload":{}}),
            ),
        );
        assert!(!app.sessions.iter().any(|(k, _)| k == "a"));
        assert!(app.sessions.iter().any(|(k, _)| k == "b"));
    }
    #[test]
    fn empty_chat_does_not_materialize_a_server_session() {
        let mut app = D4c::default();
        let (out, requests) = std::sync::mpsc::channel();
        let (_, rx) = std::sync::mpsc::channel();
        app.gateway = Some(gateway::test_gateway(out, rx));
        app.patch_selection();
        assert!(!requests.try_iter().any(|r| r["method"] == "sessions.patch"));
        assert!(app.session_settings.contains_key(&app.session));
    }
    #[test]
    fn first_send_waits_for_model_save_and_failed_save_restores_draft() {
        for success in [true, false] {
            let mut app = D4c::default();
            app.connected = true;
            app.input = "first message".into();
            app.attachments
                .push(("note.txt".into(), "text/plain".into(), b"note".to_vec()));
            let (tx, rx) = std::sync::mpsc::channel();
            let (out, requests) = std::sync::mpsc::channel();
            app.gateway = Some(gateway::test_gateway(out, rx));
            app.send(&egui::Context::default());
            let initial: Vec<_> = requests.try_iter().collect();
            assert_eq!(initial.len(), 1);
            assert_eq!(initial[0]["method"], "sessions.patch");
            tx.send(gateway::Event::Data(json!({"type":"res","id":initial[0]["id"],"ok":success,"payload":{},"error":{"message":"save failed"}}))).unwrap();
            app.events(&egui::Context::default());
            let sent: Vec<_> = requests
                .try_iter()
                .filter(|r| r["method"] == "chat.send")
                .collect();
            assert_eq!(sent.len(), usize::from(success));
            if !success {
                assert_eq!(app.input, "first message");
                assert_eq!(app.attachments[0].2, b"note");
                assert!(!app.running);
            }
        }
    }
    #[test]
    fn remote_deletion_and_late_roster_cannot_restore_the_chat() {
        let mut app = D4c::default();
        app.session = "gone".into();
        app.sessions = vec![
            ("gone".into(), "Old".into()),
            ("keep".into(), "Keep".into()),
        ];
        app.input = "discarded draft".into();
        let (tx, rx) = std::sync::mpsc::channel();
        let (out, requests) = std::sync::mpsc::channel();
        app.gateway = Some(gateway::test_gateway(out, rx));
        tx.send(gateway::Event::Data(json!({"type":"event","event":"sessions.changed","payload":{"sessionKey":"gone","reason":"delete"}}))).unwrap();
        tx.send(gateway::Event::Data(json!({"type":"res","id":"sessions.list:stale","ok":true,"payload":{"sessions":[{"key":"gone"},{"key":"keep","label":"Keep"}]}}))).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.session, "keep");
        assert!(app.input.is_empty());
        assert!(!app.sessions.iter().any(|(key, _)| key == "gone"));
        assert!(!requests.try_iter().any(|r| r["method"] == "sessions.patch"));
        app.select_session("gone".into(), false);
        assert_eq!(app.session, "keep");
    }
    #[test]
    fn deleting_selected_chat_does_not_restore_its_draft() {
        let mut app = D4c::default();
        app.session = "gone".into();
        app.input = "unsent private draft".into();
        app.attachments
            .push(("a.txt".into(), "text/plain".into(), vec![1]));
        app.delete_pending = Some(("sessions.delete:1".into(), "gone".into()));
        inject(
            &mut app,
            gateway::Event::Data(
                json!({"type":"res","id":"sessions.delete:1","ok":true,"payload":{}}),
            ),
        );
        assert_ne!(app.session, "gone");
        assert!(!app.drafts.contains_key("gone"));
        assert!(app.input.is_empty());
        assert!(app.attachments.is_empty());
    }
    #[test]
    fn disconnect_clears_stale_run_protection() {
        let mut app = D4c::default();
        app.active_sessions.insert("finished-offline".into());
        inject(&mut app, gateway::Event::Error("disconnected".into()));
        assert!(app.active_sessions.is_empty());
    }
    #[test]
    fn server_reset_discards_previous_server_content() {
        let mut app = D4c::default();
        app.preferences.connections.push(preferences::Connection {
            name: "Other".into(),
            url: "wss://other.example".into(),
        });
        app.input = "private draft".into();
        app.attachments
            .push(("private.txt".into(), "text/plain".into(), vec![1, 2]));
        app.messages.push(ChatMessage {
            media: Default::default(),
            role: "user".into(),
            text: "private".into(),
            item_id: None,
        });
        app.active_sessions.insert(app.session.clone());
        let old = app.session.clone();
        app.reset_connection(1, "explicit-token-for-new-server".into());
        assert!(app.input.is_empty());
        assert!(app.attachments.is_empty());
        assert!(app.messages.is_empty());
        assert!(app.active_sessions.is_empty());
        assert_ne!(app.session, old);
        assert_eq!(app.url, "wss://other.example");
        assert_eq!(app.token, "explicit-token-for-new-server");
    }
    #[test]
    fn hidden_compact_chat_is_not_visible() {
        assert!(!chat_is_visible(false, false, true, false, true, true));
        assert!(!chat_is_visible(false, false, false, true, true, true));
        assert!(chat_is_visible(false, false, true, true, false, true));
        assert!(!chat_is_visible(false, false, false, false, false, false));
    }
    #[test]
    fn late_history_does_not_replace_the_selected_conversation() {
        let mut app = D4c::default();
        app.session = "second".into();
        app.history_request = Some(("chat.history:second".into(), "second".into()));
        app.messages.push(ChatMessage {
            media: Default::default(),
            role: "user".into(),
            text: "Current conversation".into(),
            item_id: None,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let (out, _) = std::sync::mpsc::channel();
        app.gateway = Some(gateway::test_gateway(out, rx));
        tx.send(gateway::Event::Data(json!({"type":"res","ok":true,"id":"chat.history:first","payload":{"messages":[{"role":"user","content":"Wrong conversation"}]}}))).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.messages[0].text, "Current conversation");
        tx.send(gateway::Event::Data(json!({"type":"res","ok":true,"id":"chat.history:second","payload":{"messages":[{"role":"user","content":"Selected history"}]}}))).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.messages[0].text, "Selected history");
        assert!(app.history_request.is_none());
    }
    #[test]
    fn remote_user_message_is_visible_and_echo_is_deduplicated() {
        let mut app = D4c::default();
        app.session = "shared".into();
        let (tx, rx) = std::sync::mpsc::channel();
        let (out, _) = std::sync::mpsc::channel();
        app.gateway = Some(gateway::test_gateway(out, rx));
        let event = json!({"type":"event","event":"session.message","payload":{"sessionKey":"shared","messageId":"m1","message":{"role":"user","content":"From phone"}}});
        tx.send(gateway::Event::Data(event.clone())).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].text, "From phone");
        tx.send(gateway::Event::Data(event)).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.messages.len(), 1);
        assert!(app.history_dirty);
        tx.send(gateway::Event::Data(json!({"event":"session.message","payload":{"sessionKey":"other","messageId":"m2","message":{"role":"user","content":"Other chat"}}}))).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.messages.len(), 1);
        app.messages.push(ChatMessage {
            role: "user".into(),
            text: "Local echo".into(),
            media: vec![],
            item_id: None,
        });
        tx.send(gateway::Event::Data(json!({"event":"session.message","payload":{"sessionKey":"shared","messageId":"m3","message":{"role":"user","content":"Local echo"}}}))).unwrap();
        app.events(&egui::Context::default());
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].item_id.as_deref(), Some("message:m3"));
    }
    #[test]
    fn live_event_invalidates_an_older_background_history_snapshot() {
        let mut app = D4c::default();
        app.session = "shared".into();
        app.history_request = Some(("chat.history:old".into(), "shared".into()));
        let (tx, rx) = std::sync::mpsc::channel();
        let (out, _) = std::sync::mpsc::channel();
        app.gateway = Some(gateway::test_gateway(out, rx));
        tx.send(gateway::Event::Data(json!({"event":"session.message","payload":{"sessionKey":"shared","messageId":"new","message":{"role":"user","content":"Latest"}}}))).unwrap();
        tx.send(gateway::Event::Data(
            json!({"type":"res","ok":true,"id":"chat.history:old","payload":{"messages":[]}}),
        ))
        .unwrap();
        app.events(&egui::Context::default());
        assert!(app.history_request.is_none());
        assert_eq!(app.messages[0].text, "Latest");
    }
    #[test]
    fn drafts_and_attachments_stay_with_their_conversation() {
        let mut app = D4c::default();
        let original = app.session.clone();
        app.input = "Unsent draft".into();
        app.attachments
            .push(("note.txt".into(), "text/plain".into(), b"note".to_vec()));
        app.select_session("other".into(), false);
        assert!(app.input.is_empty());
        assert!(app.attachments.is_empty());
        app.select_session(original, false);
        assert_eq!(app.input, "Unsent draft");
        assert_eq!(app.attachments[0].2, b"note");
    }
    #[test]
    fn shift_enter_stays_available_for_a_newline() {
        let event = |modifiers| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let mut input = egui::InputState::default();
        input.events = vec![event(egui::Modifiers::SHIFT)];
        assert!(!consume_plain_key(&mut input, egui::Key::Enter));
        assert_eq!(input.events.len(), 1);
        input.events.push(event(egui::Modifiers::NONE));
        assert!(consume_plain_key(&mut input, egui::Key::Enter));
        assert_eq!(input.events.len(), 1);
    }
}

fn chat_is_visible(
    settings: bool,
    cron: bool,
    notifications: bool,
    sidebar: bool,
    compact: bool,
    focused: bool,
) -> bool {
    !settings && !cron && focused && !(compact && (notifications || sidebar))
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn recovered_run_keeps_partial_text_and_activity() {
        let mut app = D4c::default();
        app.session = "agent:main:recovery-test".into();
        app.recover_inflight(&json!({"runId":"running","text":"Still typing","events":[{"stream":"tool","data":{"name":"exec","phase":"start","toolCallId":"call-1"}}]}));
        assert!(app.running);
        assert!(app.active_sessions.contains(&app.session));
        assert_eq!(app.messages.last().unwrap().text, "Still typing");
        assert!(
            app.messages
                .iter()
                .any(|m| m.item_id.as_deref() == Some("tool:call-1"))
        );
        assert!(app.live.contains_key(&app.session));
    }
}
