use crate::{
    button_label, icon_text, preferences,
    workspace::{self, Event, Link, Workspace},
};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const MIN_DOCK_CHAT_WIDTH: f32 = 340.0;
const MIN_DOCK_VIEWER_WIDTH: f32 = 360.0;

#[derive(Default)]
pub struct Desktops {
    pub workspace: Workspace,
    pub open: bool,
    name: String,
    rename: Option<(String, String)>,
    delete: Option<String>,
    viewer: Option<Viewer>,
    focused_session: Option<String>,
    bound_desktop: Option<String>,
    layouts: HashMap<String, ViewerLayout>,
    layout_file: Option<String>,
    layout_dirty_since: Option<Instant>,
    choose_other: bool,
    pending_create: Option<PendingCreate>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct ViewerLayout {
    desktop: String,
    bubble: Option<[f32; 2]>,
    expanded: bool,
    zoom: f32,
    fullscreen: bool,
    floating: Option<[[f32; 2]; 2]>,
    docked: bool,
    dock_width: Option<f32>,
}
struct PendingCreate {
    session: String,
    name: String,
    revision: u64,
    existing: std::collections::HashSet<String>,
    started: std::time::Instant,
}
struct Viewer {
    id: String,
    name: String,
    link: Option<Link>,
    base: String,
    expanded: bool,
    bubble_pos: Option<egui::Pos2>,
    texture: Option<egui::TextureHandle>,
    width: u16,
    height: u16,
    status: String,
    waiting_for_frame: bool,
    last_claim: std::time::Instant,
    buttons: u8,
    zoom: f32,
    fullscreen: bool,
    docked: bool,
    touch_control: bool,
    text: String,
    dock_width: Option<f32>,
    initial_fit: bool,
    float_rect: Option<egui::Rect>,
}
fn desktop_skeleton(ui:&mut egui::Ui,width:u16,height:u16) {
    let available=ui.available_size();
    let size=egui::vec2(available.x.max(1.0),available.y.max(40.0).min(available.x*height as f32/width.max(1) as f32).max(1.0));
    let (rect,_)=ui.allocate_exact_size(size,egui::Sense::hover());let painter=ui.painter();
    painter.rect_filled(rect,8.0,egui::Color32::from_rgb(18,20,24));
    let pulse=(22.0+8.0*(ui.input(|i|i.time) as f32*2.4).sin()) as u8;
    let color=egui::Color32::from_white_alpha(pulse);
    let bar=|x:f32,y:f32,w:f32,h:f32|egui::Rect::from_min_size(rect.min+egui::vec2(x,y),egui::vec2(w.max(1.0),h.max(1.0))).intersect(rect);
    painter.rect_filled(bar(0.0,0.0,rect.width(),22.0),6.0,color);
    for n in 0..3 {painter.circle_filled(rect.min+egui::vec2(12.0+n as f32*10.0,11.0),2.0,egui::Color32::from_white_alpha(65));}
    if rect.height()>100.0 {
        let inner=rect.shrink2(egui::vec2(14.0,36.0));
        painter.rect_filled(inner,6.0,egui::Color32::from_white_alpha(7));
        for (n,fraction) in [0.68,0.82,0.45,0.73].into_iter().enumerate(){
            let y=inner.top()+18.0+n as f32*22.0;if y+8.0>inner.bottom(){break;}
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(inner.left()+14.0,y),egui::vec2((inner.width()-28.0)*fraction,8.0)),4.0,color);
        }
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(66));
}
impl Viewer {
    fn pause(&mut self) { self.buttons=0;self.expanded=false;self.link=None;self.waiting_for_frame=true; }
}
impl Desktops {
    fn load_layout(&mut self, url: &str) {
        use sha2::{Digest, Sha256};
        if !preferences::valid_url(url) {
            return;
        }
        let file = format!("desktop-layout-{:x}.json", Sha256::digest(url.as_bytes()));
        if self.layout_file.as_ref() != Some(&file) {
            self.save_layout(true);
            self.layouts = serde_json::from_value(preferences::read(&file)).unwrap_or_default();
            self.layout_file = Some(file);
            self.layout_dirty_since = None;
        }
    }

    fn remember_viewer_layout(&mut self) {
        let (Some(chat), Some(viewer)) = (self.focused_session.as_ref(), self.viewer.as_ref()) else {
            return;
        };
        if self.bound_desktop.as_ref() != Some(&viewer.id) {
            return;
        }
        let layout = ViewerLayout {
            desktop: viewer.id.clone(),
            bubble: viewer.bubble_pos.map(|pos| [pos.x, pos.y]),
            expanded: viewer.expanded,
            zoom: viewer.zoom,
            fullscreen: viewer.fullscreen,
            floating: viewer
                .float_rect
                .map(|rect| [[rect.min.x, rect.min.y], [rect.max.x, rect.max.y]]),
            docked: viewer.docked,
            dock_width: viewer.dock_width,
        };
        if self.layouts.get(chat) != Some(&layout) {
            self.layouts.insert(chat.clone(), layout);
            self.layout_dirty_since.get_or_insert_with(Instant::now);
        }
    }

    pub(crate) fn save_layout(&mut self, force: bool) {
        self.remember_viewer_layout();
        let (Some(file), Some(since)) = (&self.layout_file, self.layout_dirty_since) else {
            return;
        };
        if !force && since.elapsed() < Duration::from_millis(400) {
            return;
        }
        match serde_json::to_value(&self.layouts)
            .map_err(std::io::Error::from)
            .and_then(|value| preferences::write(file, &value))
        {
            Ok(()) => self.layout_dirty_since = None,
            Err(error) => {
                eprintln!("Could not save BotScreen layout: {error}");
                self.layout_dirty_since = Some(Instant::now());
            }
        }
    }

    pub fn connect(&mut self, url: &str, ctx: &egui::Context) {
        self.save_layout(true);
        self.load_layout(url);
        self.viewer = None;
        self.focused_session = None;
        self.bound_desktop = None;
        self.pending_create = None;
        self.choose_other = false;
        self.workspace.connect(url, ctx);
    }
    /// Restore only the selected chat's server-owned binding. Viewer video is
    /// client-local and never stops the remote desktop when chats are switched.
    pub fn follow_session(&mut self, session: &str, url: &str, ctx: &egui::Context) {
        if !self.workspace.ready {
            return;
        }
        if self.layout_file.is_none() {
            self.load_layout(url);
        }
        let desktop = self
            .workspace
            .get("binding", session)
            .filter(|b| b["deleted"] != true)
            .and_then(|b| b["desktopId"].as_str())
            .and_then(|id| self.workspace.get("desktop", id))
            .filter(|d| d["deleted"] != true)
            .cloned();
        let id = desktop
            .as_ref()
            .and_then(|d| d["id"].as_str())
            .map(str::to_owned);
        if self.focused_session.as_deref() == Some(session) && self.bound_desktop == id {
            return;
        }
        self.remember_viewer_layout();
        if let Some(old) = self.viewer.take() {
            if old.buttons != 0 {
                self.workspace.rpc(
                    "desktop.input",
                    json!({"id":old.id,"type":"pointer","x":0,"y":0,"buttons":0}),
                );
            }
        }
        self.choose_other = false;
        self.focused_session = Some(session.into());
        self.bound_desktop = id;
        if let Some(desktop) = desktop {
            self.view(&desktop, url, ctx);
        }
    }
    pub fn poll_ui(&mut self, ctx: &egui::Context) {
        self.workspace.poll();
        if let Some(p) = self.pending_create.as_ref() {
            let created = self.workspace.list("desktop").into_iter().find(|d| {
                d["name"].as_str() == Some(&p.name)
                    && d["id"].as_str().is_some_and(|id| !p.existing.contains(id))
            });
            if let Some(desktop) = created {
                let p = self.pending_create.take().unwrap();
                self.workspace.rpc("desktop.bind",json!({"session":p.session,"desktopId":desktop["id"],"expectedRevision":p.revision}));
                self.choose_other = false;
            } else if p.started.elapsed() > std::time::Duration::from_secs(45) {
                self.pending_create = None;
                self.workspace.error =
                    "Desktop creation was not confirmed. Refresh before retrying.".into();
            }
        }
        if let Some(v) = self.viewer.as_mut() {
            let Some(link) = v.link.as_ref() else {
                return;
            };
            let mut events: Vec<_> = link.rx.try_iter().collect();
            if let Some((w, h, pixels)) = link.frame.lock().ok().and_then(|mut f| f.take()) {
                events.push(Event::Frame(w, h, pixels));
            }
            for e in events {
                match e {
                    Event::Status(s) => {
                        v.waiting_for_frame = true;
                        v.status = if s == "Online" {
                            "Loading desktop…".into()
                        } else {
                            s
                        };
                    }
                    Event::Frame(w, h, pixels) => {
                        v.waiting_for_frame = false;
                        v.status = "Online".into();
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [w as usize, h as usize],
                            &pixels,
                        );
                        if let Some(t) = v.texture.as_mut() {
                            t.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            v.texture = Some(ctx.load_texture(
                                format!("vnc-{}", v.id),
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        v.width = w;
                        v.height = h;
                    }
                    _ => {}
                }
            }
        }
    }
    pub fn view(&mut self, d: &Value, url: &str, ctx: &egui::Context) {
        ctx.memory_mut(|memory| {
            if let Some(focus) = memory.focused() {
                memory.surrender_focus(focus);
            }
        });
        let Some(base) = workspace::base(url) else {
            return;
        };
        let id = d["id"].as_str().unwrap_or("").to_string();
        self.remember_viewer_layout();
        let layout = self
            .focused_session
            .as_ref()
            .and_then(|session| self.layouts.get(session))
            .filter(|layout| layout.desktop == id);
        let expanded = layout.is_some_and(|layout| layout.expanded);
        let link = expanded.then(|| workspace::connect(base.clone(), Some(id.clone()), ctx.clone()));
        self.viewer = Some(Viewer {
            id,
            name: d["name"].as_str().unwrap_or("Desktop").into(),
            link,
            base,
            expanded,
            bubble_pos: layout
                .and_then(|layout| layout.bubble)
                .map(|[x, y]| egui::pos2(x, y)),
            texture: None,
            width: 1280,
            height: 800,
            status: "Connecting…".into(),
            waiting_for_frame: true,
            last_claim: std::time::Instant::now(),
            buttons: 0,
            zoom: layout.map_or(1.0, |layout| layout.zoom.clamp(1.0, 5.0)),
            fullscreen: layout.is_some_and(|layout| layout.fullscreen),
            docked: layout.is_some_and(|layout| layout.docked),
            touch_control: false,
            text: String::new(),
            dock_width: layout.and_then(|layout| layout.dock_width),
            initial_fit: true,
            float_rect: layout
                .and_then(|layout| layout.floating)
                .map(|[[x, y], [right, bottom]]| {
                    egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(right, bottom))
                }),
        });
    }
    pub fn slash(&mut self, text: &str, session: &str, url: &str, ctx: &egui::Context) -> bool {
        let Some(command) = text.trim().strip_prefix("/vnc") else {
            return false;
        };
        if !command.is_empty() && !command.starts_with(char::is_whitespace) {
            return false;
        }
        let command = command.trim();
        let revision = self
            .workspace
            .get("binding", session)
            .and_then(|v| v["rev"].as_u64())
            .unwrap_or(0);
        if command == "disconnect" {
            self.workspace.rpc(
                "desktop.bind",
                json!({"session":session,"expectedRevision":revision}),
            );
            self.viewer = None;
            return true;
        }
        if let Some(name) = command.strip_prefix("connect ") {
            if let Some(d) = self.workspace.list("desktop").into_iter().find(|d| {
                d["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name.trim()))
            }) {
                self.workspace.rpc(
                    "desktop.bind",
                    json!({"session":session,"desktopId":d["id"],"expectedRevision":revision}),
                );
                self.view(&d, url, ctx);
            } else {
                self.workspace.error = "No desktop with that name".into();
            }
        } else {
            self.open = true;
        }
        true
    }
    pub fn suggestions(&self, input: &str) -> Vec<crate::commands::Suggestion> {
        let items = std::iter::once((
            "/vnc disconnect".to_string(),
            "Detach from this chat".to_string(),
        ))
        .chain(self.workspace.list("desktop").into_iter().map(|d| {
            (
                format!("/vnc connect {}", d["name"].as_str().unwrap_or("")),
                format!("Open {}", d["name"].as_str().unwrap_or("desktop")),
            )
        }));
        if input == "/" || "/vnc".starts_with(input) && input.starts_with('/') {
            return vec![crate::commands::Suggestion {
                label: "/vnc".into(),
                description: "Connect a server desktop".into(),
                replacement: "/vnc connect ".into(),
                skill: false,
            }];
        }
        if !input.starts_with("/vnc ") {
            return vec![];
        }
        items
            .filter(|(label, _)| label.to_lowercase().starts_with(&input.to_lowercase()))
            .take(20)
            .map(|(label, description)| crate::commands::Suggestion {
                replacement: label.clone(),
                label,
                description,
                skill: false,
            })
            .collect()
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, url: &str, session: &str) {
        let bound = self
            .workspace
            .get("binding", session)
            .and_then(|b| b["desktopId"].as_str())
            .map(str::to_owned);
        let busy = self
            .workspace
            .get("activity", session)
            .is_some_and(|v| v["running"] == true);
        if bound.is_some()
            && !self.choose_other
            && ui
                .add_enabled(!busy, egui::Button::new("Change desktop"))
                .clicked()
        {
            self.choose_other = true;
        }
        if bound.is_none() || self.choose_other {
            ui.add_space(10.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text("New desktop name")
                    .desired_width(f32::INFINITY),
            );
            if ui
                .add_enabled(
                    !busy && self.pending_create.is_none() && !self.name.trim().is_empty(),
                    egui::Button::new(button_label(lucide_icons::Icon::Plus, "Create and connect"))
                        .min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked()
            {
                let name = self.name.trim().to_owned();
                self.pending_create = Some(PendingCreate {
                    session: session.into(),
                    name: name.clone(),
                    revision: self
                        .workspace
                        .get("binding", session)
                        .and_then(|b| b["rev"].as_u64())
                        .unwrap_or(0),
                    existing: self
                        .workspace
                        .list("desktop")
                        .iter()
                        .filter_map(|d| d["id"].as_str().map(str::to_owned))
                        .collect(),
                    started: std::time::Instant::now(),
                });
                self.workspace.rpc("desktop.create", json!({"name":name}));
                self.name.clear();
            }
            if self.pending_create.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Creating desktop…");
                });
            }
            ui.add_space(12.0);
        }
        if busy {
            ui.label(
                egui::RichText::new("The agent is working. Connection changes are paused.")
                    .small()
                    .weak(),
            );
        }
        ui.add_space(8.0);
        if self.workspace.status != "Online" && !self.workspace.ready {
            ui.label(egui::RichText::new(&self.workspace.status).small().weak());
        }
        if !self.workspace.error.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &self.workspace.error);
            if ui.small_button("Dismiss").clicked() {
                self.workspace.error.clear();
            }
        }
        egui::ScrollArea::vertical().show(ui,|ui|{for d in self.workspace.list("desktop"){
   if bound.is_some() && !self.choose_other && bound.as_deref()!=d["id"].as_str(){continue;}
   let id=d["id"].as_str().unwrap_or("").to_string();let name=d["name"].as_str().unwrap_or("Desktop");let status=d["status"].as_str().unwrap_or("new");
   egui::Frame::new().fill(egui::Color32::from_rgb(29,31,36)).corner_radius(12).inner_margin(16).show(ui,|ui|{
    ui.set_min_width(ui.available_width().max(1.0));ui.horizontal_wrapped(|ui|{ui.label(icon_text(lucide_icons::Icon::Monitor,20.0));ui.strong(name);ui.label(egui::RichText::new(status).small().weak());});ui.add_space(10.0);
    ui.horizontal_wrapped(|ui|{if status=="running"{if ui.button("Open").clicked(){self.view(&d,url,ui.ctx());}if bound.as_deref()!=Some(id.as_str()) && ui.add_enabled(!busy,egui::Button::new("Connect to chat")).clicked(){let rev=self.workspace.get("binding",session).and_then(|v|v["rev"].as_u64()).unwrap_or(0);self.workspace.rpc("desktop.bind",json!({"session":session,"desktopId":id,"expectedRevision":rev}));self.choose_other=false;}
     if bound.as_deref()==Some(id.as_str()) && ui.add_enabled(!busy,egui::Button::new("Disconnect")).clicked(){let rev=self.workspace.get("binding",session).and_then(|v|v["rev"].as_u64()).unwrap_or(0);self.workspace.rpc("desktop.bind",json!({"session":session,"expectedRevision":rev}));}}
     if matches!(status,"running"|"stopped"|"error"|"new")&&ui.button(if status=="running"{"Stop"}else{"Start"}).clicked(){self.workspace.rpc(if status=="running"{"desktop.stop"}else{"desktop.start"},json!({"id":id,"expectedRevision":d["rev"]}));}
     ui.menu_button("More",|ui|{if ui.button("Rename").clicked(){self.rename=Some((id.clone(),name.into()));ui.close();}if ui.button("Delete…").clicked(){self.delete=Some(id.clone());ui.close();}});
    });if let Some(e)=d["lastError"].as_str(){ui.colored_label(egui::Color32::LIGHT_RED,e);}
    if let Some((rename_id,text))=self.rename.as_mut(){if rename_id==&id{ui.text_edit_singleline(text);let new=text.clone();ui.horizontal(|ui|{if ui.button("Save").clicked(){self.workspace.rpc("desktop.rename",json!({"id":id,"name":new,"expectedRevision":d["rev"]}));self.rename=None;}if ui.button("Cancel").clicked(){self.rename=None;}});}}
    if self.delete.as_ref()==Some(&id){ui.label("Stop and remove this desktop? Its files will be retained on the server.");ui.horizontal(|ui|{if ui.button("Delete desktop").clicked(){self.workspace.rpc("desktop.delete",json!({"id":id,"confirm":true,"expectedRevision":d["rev"]}));self.delete=None;}if ui.button("Keep").clicked(){self.delete=None;}});}
   });ui.add_space(10.0);
  }});
    }
    pub fn dock_width(ui: &egui::Ui) -> Option<f32> {
        let bounds = crate::platform::content_rect(ui.ctx());
        let narrow_portrait = bounds.width() < 760.0 && bounds.width() < bounds.height();
        let width = ui.available_width();
        (!narrow_portrait && width >= MIN_DOCK_CHAT_WIDTH + MIN_DOCK_VIEWER_WIDTH)
            .then(|| (width * 0.75).min(width - MIN_DOCK_CHAT_WIDTH))
    }

    pub fn viewer_dock_ui(&mut self, ui: &mut egui::Ui, width: Option<f32>) -> bool {
        if let Some(width) = width
            && self.viewer.as_ref().is_some_and(|v| v.expanded && v.docked && !v.fullscreen)
        {
            let ctx = ui.ctx().clone();
            self.show_viewer(&ctx, Some((ui, width)), true);
            return true;
        }
        false
    }

    pub fn viewer_ui(&mut self, ctx: &egui::Context, can_dock: bool) {
        self.show_viewer(ctx, None, can_dock);
    }

    fn show_viewer(
        &mut self,
        ctx: &egui::Context,
        dock: Option<(&mut egui::Ui, f32)>,
        can_dock: bool,
    ) {
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        let d = self
            .workspace
            .get("desktop", &v.id)
            .cloned()
            .unwrap_or(Value::Null);
        let stream_live = v.link.as_ref().is_some_and(Link::is_online) && !v.waiting_for_frame;
        if !stream_live && v.buttons != 0 {
            self.workspace.rpc(
                "desktop.input",
                json!({"id":v.id,"type":"pointer","x":0,"y":0,"buttons":0}),
            );
            v.buttons = 0;
        }
        let human = d["mode"] == "human";
        let pending_control = self.workspace.control_pending(&v.id);
        let mut own = stream_live
            && human
            && !pending_control
            && d["controller"].as_str() == self.workspace.device.as_deref();
        let in_use = self
            .workspace
            .list("binding")
            .iter()
            .filter(|b| b["desktopId"] == v.id)
            .any(|b| {
                b["id"]
                    .as_str()
                    .and_then(|key| self.workspace.get("activity", key))
                    .is_some_and(|a| a["running"] == true)
            });
        let bounds = crate::platform::content_rect(ctx);
        let mobile = bounds.width() < 640.0;
        if !v.expanded {
            // Floating overlay: preserves full chat width and the user's drag position.
            let diameter = 52.0;
            let drag_bounds = bounds.shrink(6.0);
            let clamp_position = |pos: egui::Pos2| {
                egui::pos2(
                    pos.x.clamp(
                        drag_bounds.left(),
                        (drag_bounds.right() - diameter).max(drag_bounds.left()),
                    ),
                    pos.y.clamp(
                        drag_bounds.top(),
                        (drag_bounds.bottom() - diameter).max(drag_bounds.top()),
                    ),
                )
            };
            let mut pos = clamp_position(v.bubble_pos.unwrap_or_else(|| {
                egui::pos2(bounds.right() - diameter - 12.0, bounds.center().y)
            }));

            let mut expand = false;
            let mut dismiss = false;
            egui::Area::new(egui::Id::new(("vnc_bubble", &v.id)))
                .order(egui::Order::Foreground)
                .movable(false)
                .fixed_pos(pos)
                .show(ctx, |ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(diameter, diameter),
                        egui::Sense::click_and_drag(),
                    );
                    let center = rect.center();
                    ui.painter().circle_filled(
                        center + egui::vec2(0.0, 3.0),
                        diameter / 2.0 + 1.0,
                        egui::Color32::from_black_alpha(60),
                    );
                    ui.painter().circle_filled(
                        center,
                        diameter / 2.0 - 1.0,
                        egui::Color32::from_rgb(30, 35, 40),
                    );
                    ui.painter().circle_stroke(
                        center,
                        diameter / 2.0 - 2.0,
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(131, 212, 189)),
                    );
                    ui.painter().text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        char::from(lucide_icons::Icon::Monitor),
                        egui::FontId::new(
                            if mobile { 20.0 } else { 23.0 },
                            egui::FontFamily::Name("lucide".into()),
                        ),
                        egui::Color32::from_rgb(207, 235, 228),
                    );
                    let offset = diameter * 0.34;
                    let dot = center + egui::vec2(offset, -offset);
                    ui.painter()
                        .circle_filled(dot, 5.0, egui::Color32::from_rgb(30, 35, 40));
                    ui.painter().circle_filled(
                        dot,
                        3.0,
                        if human {
                            egui::Color32::from_rgb(223, 187, 121)
                        } else {
                            egui::Color32::from_rgb(131, 212, 189)
                        },
                    );
                    if response.dragged() {
                        pos = clamp_position(pos + response.drag_delta());
                        v.bubble_pos = Some(pos);
                        ctx.request_repaint();
                    }
                    expand = response.clicked();
                    response.context_menu(|ui| {
                        ui.label(&v.name);
                        if ui.button("Dismiss bubble").clicked() {
                            dismiss = true;
                            ui.close();
                        }
                    });
                });
            if dismiss {
                self.viewer = None;
            } else if expand {
                ctx.memory_mut(|m| {
                    if let Some(focus) = m.focused() {
                        m.surrender_focus(focus);
                    }
                });
                v.expanded = true;
                v.initial_fit = true;
                v.status = "Connecting…".into();
                v.waiting_for_frame = true;
                v.link = Some(workspace::connect(
                    v.base.clone(),
                    Some(v.id.clone()),
                    ctx.clone(),
                ));
                ctx.request_repaint();
            }
            return;
        }
        let mut open = true;
        let mut close = false;
        let was_fullscreen = v.fullscreen;
        let floating = dock.is_none();
        let mut content = |ui: &mut egui::Ui, v: &mut Viewer| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            let button_size = if mobile { 32.0 } else { 28.0 };
            let single_row = ui.available_width() >= 570.0 && !mobile;
            let control_status = if own {
                "Your control"
            } else if human {
                "Human control"
            } else if in_use {
                "Computer in use"
            } else {
                "Agent mode"
            };
            let status = |ui: &mut egui::Ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(control_status).size(11.0).color(
                        if human {
                            egui::Color32::from_rgb(223, 187, 121)
                        } else {
                            egui::Color32::from_rgb(143, 191, 178)
                        },
                    ))
                    .truncate(),
                );
            };
            let icon_button = |ui: &mut egui::Ui, icon, tooltip: &str| {
                ui.add(
                    egui::Button::new(icon_text(icon, 15.0))
                        .frame(false)
                        .min_size(egui::Vec2::splat(button_size)),
                )
                .on_hover_text(tooltip)
                .clicked()
            };
            // Both rows reserve room for the controls before laying out text.
            let view_controls = |ui: &mut egui::Ui, v: &mut Viewer| {
                if mobile && own
                    && ui
                        .add(
                            egui::Button::selectable(
                                v.touch_control,
                                egui::RichText::new(if v.touch_control { "Touch" } else { "Pan" })
                                    .size(11.0),
                            )
                            .min_size(egui::vec2(0.0, button_size)),
                        )
                        .on_hover_text(if v.touch_control { "Touch control" } else { "Swipe to pan" })
                        .clicked()
                {
                    v.touch_control = !v.touch_control;
                }
                if can_dock && !v.fullscreen {
                    let (icon, label) = if floating {
                        (lucide_icons::Icon::PanelRightOpen, "Dock right")
                    } else {
                        (lucide_icons::Icon::PictureInPicture, "Float")
                    };
                    if icon_button(ui, icon, label) {
                        v.docked = floating;
                        ctx.request_repaint();
                    }
                }
                if icon_button(ui, lucide_icons::Icon::Plus, "Zoom in") {
                    v.zoom = (v.zoom * 1.4).min(5.0);
                }
                if ui.add(
                    egui::Button::new(egui::RichText::new(format!("Fit  {:.0}%", v.zoom * 100.0)).size(11.0))
                        .frame(false)
                        .min_size(egui::vec2(64.0, button_size)),
                )
                .on_hover_text("Fit to viewer")
                .clicked()
                {
                    v.zoom = 1.0;
                }
                if icon_button(ui, lucide_icons::Icon::Minus, "Zoom out") {
                    v.zoom = (v.zoom / 1.4).max(1.0);
                }
            };
            let mut control_changed = false;
            let row_size = egui::vec2(ui.available_width(), button_size);
            let row_layout = egui::Layout::right_to_left(egui::Align::Center);
            let title_layout = egui::Layout::left_to_right(egui::Align::Center);
            ui.allocate_ui_with_layout(row_size, row_layout, |ui| {
                if icon_button(ui, lucide_icons::Icon::Minus, "Minimize to bubble") {
                    close = true;
                }
                let (fullscreen_icon, fullscreen_label) = if v.fullscreen {
                    (lucide_icons::Icon::Minimize, "Exit fullscreen")
                } else {
                    (lucide_icons::Icon::Maximize, "Fullscreen")
                };
                if icon_button(ui, fullscreen_icon, fullscreen_label) {
                    v.fullscreen = !v.fullscreen;
                    ctx.request_repaint();
                }
                if icon_button(ui, lucide_icons::Icon::RotateCw, &v.status) {
                    if let Some(link) = v.link.as_ref() {
                        link.reconnect();
                    }
                    v.waiting_for_frame = true;
                    v.status = "Reconnecting…".into();
                }
                if ui
                    .add_enabled(
                        !pending_control && self.workspace.ready && self.workspace.status == "Online"
                            && (stream_live || human),
                        egui::Button::new(
                            egui::RichText::new(if human { "Return to agent" } else { "Take over" })
                                .size(11.0),
                        )
                        .min_size(egui::vec2(92.0, button_size)),
                    )
                    .clicked()
                {
                    self.workspace.rpc(
                        if human { "desktop.return_agent" } else { "desktop.claim" },
                        json!({"id":v.id}),
                    );
                    v.last_claim = std::time::Instant::now();
                    v.buttons = 0;
                    control_changed = true;
                }
                if single_row {
                    ui.add_space(8.0);
                    view_controls(ui, v);
                    ui.add_space(8.0);
                }
                ui.with_layout(title_layout, |ui| {
                    ui.label(icon_text(lucide_icons::Icon::Monitor, 15.0));
                    let title =
                        egui::Label::new(egui::RichText::new(&v.name).size(14.0).strong())
                            .truncate();
                    if single_row {
                        let width = (ui.available_width() - 104.0).max(0.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, button_size),
                            title_layout,
                            |ui| ui.add(title),
                        );
                        ui.add_space(4.0);
                        status(ui);
                    } else {
                        ui.add(title);
                    }
                });
            });
            if !single_row {
                ui.allocate_ui_with_layout(row_size, row_layout, |ui| {
                    view_controls(ui, v);
                    ui.with_layout(title_layout, status);
                });
            }
            if control_changed {
                own = false;
            }
            ui.separator();
            if own {
                ui.horizontal(|ui|{
                    let edit=ui.add(egui::TextEdit::singleline(&mut v.text).hint_text("Type on desktop…").desired_width((ui.available_width()-92.0).max(40.0)));
                    if ui.add_enabled(!v.text.is_empty(),egui::Button::new(icon_text(lucide_icons::Icon::Send,18.0)).min_size(egui::vec2(36.0,36.0))).on_hover_text("Type text").clicked() || edit.lost_focus()&&ui.input(|i|i.key_pressed(egui::Key::Enter)){
                        self.workspace.rpc("desktop.input",json!({"id":v.id,"type":"text","text":std::mem::take(&mut v.text)}));
                    }
                    if ui.add(egui::Button::new(icon_text(lucide_icons::Icon::CornerDownLeft,18.0)).min_size(egui::vec2(36.0,36.0))).on_hover_text("Enter").clicked(){self.workspace.rpc("desktop.input",json!({"id":v.id,"type":"batch","events":[{"type":"key","key":65293,"down":true},{"type":"key","key":65293,"down":false}]}));}
                });
                if v.last_claim.elapsed()>std::time::Duration::from_secs(10){self.workspace.rpc("desktop.claim",json!({"id":v.id}));v.last_claim=std::time::Instant::now();}
            }
            if !stream_live {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(if v.texture.is_some(){"Reconnecting"}else{"Connecting"}).size(11.0).weak());
                    crate::loading_bar(ui, 70.0, 3.0);
                });
                if v.status.starts_with("Approve") || v.status.contains("authentication") {ui.colored_label(egui::Color32::LIGHT_RED,&v.status);}
            }
            if v.texture.is_none() {desktop_skeleton(ui,v.width,v.height);}
            if !self.workspace.error.is_empty(){ui.colored_label(egui::Color32::LIGHT_RED,&self.workspace.error);}
            if let Some(texture)=v.texture.as_ref(){
                let available = (ui.available_size() - egui::vec2(0.0, 8.0)).max(egui::Vec2::splat(1.0));
                let ratio=(available.x/v.width as f32).min(available.y/v.height as f32)*v.zoom;
                let size=egui::vec2(v.width as f32*ratio,v.height as f32*ratio);
                egui::ScrollArea::both().id_salt(("vnc_pan",&v.id)).auto_shrink([false,false]).scroll_source(egui::scroll_area::ScrollSource{scroll_bar:true,drag:if mobile&&!v.touch_control||!own{egui::scroll_area::DragScroll::Always}else{egui::scroll_area::DragScroll::Never},mouse_wheel:!own||mobile&&!v.touch_control}).show(ui,|ui|{
                    let interactive=own && (!mobile||v.touch_control);
                    let (viewport, _) = ui.allocate_exact_size(size.max(available), egui::Sense::hover());
                    let image_rect = egui::Rect::from_center_size(viewport.center(), size);
                    let response=ui.place(image_rect, egui::Image::new((texture.id(),size)).tint(if stream_live {egui::Color32::WHITE}else{egui::Color32::from_gray(145)}).sense(if interactive{egui::Sense::click_and_drag()}else{egui::Sense::hover()}));
                    if interactive {
                        if response.clicked()||response.drag_started(){response.request_focus();}
                        let events=ctx.input(|i|i.events.clone());
                        let packets = pointer_packets(
                            &events,
                            response.rect,
                            ui.clip_rect(),
                            ratio,
                            (v.width, v.height),
                            &mut v.buttons,
                            ctx.input(|i| i.pointer.hover_pos()),
                        );
                        for events in packets.chunks(64){if !events.is_empty(){self.workspace.rpc("desktop.input",json!({"id":v.id,"type":"batch","events":events}));}}
                        if response.has_focus(){for event in events{match event{
                            egui::Event::Text(text)|egui::Event::Paste(text)=>self.workspace.rpc("desktop.input",json!({"id":v.id,"type":"text","text":text})),
                            egui::Event::Key{key,pressed,modifiers,..} if pressed=>{
                                let name=format!("{key:?}").replace("Arrow","");
                                if (name.len()!=1&&name!="Space")||modifiers.ctrl||modifiers.alt {
                                    if let Some(key)=crate::rfb::named_key(&name){
                                        let mods=[(0xffe1,modifiers.shift),(0xffe3,modifiers.ctrl),(0xffe9,modifiers.alt),(0xffeb,modifiers.mac_cmd)];let mut keys=vec![];
                                        for(k,down)in mods{if down{keys.push(json!({"type":"key","key":k,"down":true}));}}
                                        keys.push(json!({"type":"key","key":key,"down":true}));keys.push(json!({"type":"key","key":key,"down":false}));
                                        for(k,down)in mods.into_iter().rev(){if down{keys.push(json!({"type":"key","key":k,"down":false}));}}
                                        self.workspace.rpc("desktop.input",json!({"id":v.id,"type":"batch","events":keys}));
                                    }
                                }
                            },_=>{}
                        }}}
                    }
                });
            }
        };
        if let Some((ui, width)) = dock {
            let available = ui.available_width();
            let max_width = available - MIN_DOCK_CHAT_WIDTH;
            let width = v.dock_width.unwrap_or(width).clamp(MIN_DOCK_VIEWER_WIDTH, max_width);
            let salt = (
                "desktop_viewer_dock",
                &v.base,
                self.focused_session.as_deref(),
                &v.id,
            );
            let id = ui.scope_id().with_salt(egui::IdSalt::new(salt));
            let mut rect = ui.available_rect_before_wrap();
            rect.min.x = rect.right() - width;
            // Seed the panel from the preferred width so temporary viewport constraints
            // do not replace the user's setting.
            ctx.data_mut(|data| {
                data.insert_persisted(id, egui::containers::panel::PanelState { outer_rect: rect });
            });
            egui::Panel::right(salt)
                .default_size(width)
                .size_range(MIN_DOCK_VIEWER_WIDTH..=max_width)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(25, 27, 32))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 57, 63)))
                        .inner_margin(10),
                )
                .show(ui, |ui| content(ui, v));
            let actual_width = available - ui.available_width();
            if (actual_width - width).abs() > 1.0 {
                v.dock_width = Some(actual_width);
            }
        } else {
            let mobile_width = (bounds.width() - 24.0).max(260.0);
            let default_size = if mobile {
                egui::vec2(
                    mobile_width,
                    ((mobile_width - 20.0) * v.height as f32 / v.width as f32 + 112.0)
                        .min(bounds.height() - 24.0),
                )
            } else {
                egui::vec2(
                    bounds.width().min(900.0) - 16.0,
                    bounds.height().min(640.0) - 16.0,
                )
            };
            let mut floating = egui::Window::new(&v.name)
                .id(egui::Id::unique((
                    "desktop_viewer",
                    &v.base,
                    self.focused_session.as_deref(),
                    &v.id,
                    mobile,
                    was_fullscreen,
                    v.docked,
                )))
                .open(&mut open)
                .title_bar(false)
                .drag_area(egui::containers::WindowDrag::Anywhere)
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(25, 27, 32))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 57, 63)))
                        .inner_margin(10)
                        .corner_radius(14),
                )
                .constrain_to(bounds)
                .resizable(!v.fullscreen)
                .min_size([260.0, 220.0])
                .default_size(default_size);
            if let Some(rect) = v.float_rect {
                floating = floating.default_rect(egui::Rect::from_min_size(
                    rect.min,
                    rect.size().min(bounds.size()),
                ));
            } else if mobile && v.initial_fit && !was_fullscreen {
                floating = floating.fixed_size(default_size);
            }
            v.initial_fit = false;
            if was_fullscreen {
                floating = floating
                    .fixed_rect(bounds.shrink(2.0))
                    .movable(false)
                    .title_bar(false);
            }
            if let Some(shown) = floating.show(ctx, |ui| content(ui, v))
                && !was_fullscreen
                && !v.docked
            {
                v.float_rect = Some(shown.response.rect);
            }
        }
        if !open || close {
            if own {
                self.workspace.rpc(
                    "desktop.input",
                    json!({"id":v.id,"type":"pointer","x":0,"y":0,"buttons":0}),
                );
            }
            v.pause();
            ctx.request_repaint();
        }
    }
}

fn pointer_packets(
    events: &[egui::Event],
    rect: egui::Rect,
    clip_rect: egui::Rect,
    ratio: f32,
    (width, height): (u16, u16),
    buttons: &mut u8,
    pointer: Option<egui::Pos2>,
) -> Vec<Value> {
    let mut out = vec![];
    let mut pending = None;
    let visible = rect.intersect(clip_rect);
    let mut position = pointer.unwrap_or(egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY));
    let point = |pos: egui::Pos2, mask: u8| json!({"type":"pointer","x":((pos.x-rect.left())/ratio).clamp(0.0,width as f32-1.0) as u16,"y":((pos.y-rect.top())/ratio).clamp(0.0,height as f32-1.0)as u16,"buttons":mask});
    for event in events {
        match event {
            egui::Event::PointerMoved(pos) => {
                position = *pos;
                if visible.contains(*pos) || *buttons != 0 {
                    pending = Some(point(*pos, *buttons));
                }
            }
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } => {
                position = *pos;
                let bit = match button {
                    egui::PointerButton::Primary => 1,
                    egui::PointerButton::Middle => 2,
                    egui::PointerButton::Secondary => 4,
                    _ => 0,
                };
                if bit == 0
                    || (*pressed && !visible.contains(*pos))
                    || (!*pressed && *buttons & bit == 0)
                {
                    continue;
                }
                if let Some(p) = pending.take() {
                    out.push(p);
                }
                if *pressed {
                    *buttons |= bit;
                } else {
                    *buttons &= !bit;
                }
                out.push(point(*pos, *buttons));
            }
            egui::Event::MouseWheel { delta, .. } if visible.contains(position) && delta.y != 0.0 => {
                if let Some(p) = pending.take() {
                    out.push(p);
                }
                let mask = if delta.y > 0.0 { 8 } else { 16 };
                for _ in 0..((delta.y.abs() / 40.0).round() as usize).clamp(1, 6) {
                    out.push(point(position, *buttons | mask));
                    out.push(point(position, *buttons));
                }
            }
            egui::Event::PointerGone | egui::Event::WindowFocused(false) if *buttons != 0 => {
                pending = None;
                *buttons = 0;
                out.push(point(position, 0));
            }
            _ => {}
        }
    }
    if let Some(p) = pending {
        out.push(p);
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    fn bound_chats() -> Desktops {
        let mut desktops = Desktops::default();
        desktops.workspace.ready = true;
        for (session, id) in [("chat-a", "desktop-a"), ("chat-b", "desktop-b")] {
            desktops.workspace.objects.insert(
                ("binding".into(), session.into()),
                json!({"id":session,"desktopId":id}),
            );
            desktops.workspace.objects.insert(
                ("desktop".into(), id.into()),
                json!({"id":id,"name":id,"status":"running"}),
            );
        }
        desktops
    }
    fn layout_test_input(width: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            ..Default::default()
        }
    }
    #[test]
    fn restart_restores_layout_only_for_the_matching_chat_and_desktop() {
        let url = "wss://desktop-layout-restart.test";
        let ctx = egui::Context::default();
        let mut before = bound_chats();
        before.follow_session("chat-a", url, &ctx);
        let viewer = before.viewer.as_mut().unwrap();
        viewer.expanded = true;
        viewer.docked = true;
        viewer.bubble_pos = Some(egui::pos2(50.0, 80.0));
        viewer.zoom = 1.4;
        viewer.dock_width = Some(950.0);
        let rect = egui::Rect::from_min_size(egui::pos2(80.0, 120.0), egui::vec2(680.0, 480.0));
        viewer.float_rect = Some(rect);
        before.save_layout(true);

        let mut after = bound_chats();
        let fresh_ctx = egui::Context::default();
        after.follow_session("chat-b", url, &fresh_ctx);
        assert!(!after.viewer.as_ref().unwrap().expanded);
        after.follow_session("chat-a", url, &fresh_ctx);
        let viewer = after.viewer.as_ref().unwrap();
        assert!(viewer.expanded && viewer.docked);
        assert_eq!(viewer.bubble_pos, Some(egui::pos2(50.0, 80.0)));
        assert_eq!(viewer.zoom, 1.4);
        assert_eq!(viewer.dock_width, Some(950.0));
        assert_eq!(viewer.float_rect, Some(rect));
        assert!(viewer.link.is_some());

        for (width, expected_chat) in [(1900.0, 950.0), (1050.0, 340.0), (1900.0, 950.0)] {
            for _ in 0..2 {
                fresh_ctx.run_ui(layout_test_input(width), |ui| {
                    let width = Desktops::dock_width(ui);
                    assert!(after.viewer_dock_ui(ui, width));
                    assert!((ui.available_width() - expected_chat).abs() <= 1.0);
                }).textures_delta.clear();
            }
        }
        assert_eq!(after.viewer.as_ref().unwrap().dock_width, Some(950.0));
        after.viewer.as_mut().unwrap().docked = false;
        for _ in 0..3 {
            fresh_ctx.run_ui(layout_test_input(1900.0), |ui| {
                after.viewer_ui(ui.ctx(), true);
            }).textures_delta.clear();
        }
        let actual_rect = after.viewer.as_ref().unwrap().float_rect.unwrap();
        assert!((actual_rect.min - rect.min).length() <= 1.0, "{actual_rect}");
        assert!((actual_rect.size() - rect.size()).length() <= 1.0, "{actual_rect}");

        after.workspace.objects.insert(
            ("binding".into(), "chat-a".into()),
            json!({"id":"chat-a","desktopId":"desktop-b"}),
        );
        after.follow_session("chat-a", url, &fresh_ctx);
        let viewer = after.viewer.as_ref().unwrap();
        assert!(!viewer.expanded && !viewer.docked);
        assert_eq!(viewer.zoom, 1.0);
    }
    #[test]
    fn minimized_viewer_keeps_its_last_image() {
        let ctx=egui::Context::default();let mut d=Desktops::default();
        d.view(&json!({"id":"test","name":"Desktop"}),"wss://a.example",&ctx);
        let v=d.viewer.as_mut().unwrap();v.texture=Some(ctx.load_texture("retained",egui::ColorImage::new([1,1],vec![egui::Color32::BLACK]),egui::TextureOptions::LINEAR));v.expanded=true;v.pause();
        assert!(v.texture.is_some());assert!(!v.expanded);assert!(v.waiting_for_frame);assert!(v.link.is_none());
    }
    #[test]
    fn viewer_scales_the_image_when_the_viewport_changes() {
        let ctx=egui::Context::default();let mut d=Desktops::default();
        d.view(&json!({"id":"test","name":"Desktop"}),"wss://a.example",&ctx);
        let v = d.viewer.as_mut().unwrap();
        v.expanded = true;
        let texture = ctx.load_texture(
            "test desktop",
            egui::ColorImage::filled([2, 2], egui::Color32::BLACK),
            egui::TextureOptions::LINEAR,
        );
        let mut first = egui::Vec2::ZERO;
        let mut floating_image = egui::Vec2::ZERO;
        for frame in 0..60 {
            if frame == 6 {
                d.viewer.as_mut().unwrap().texture = Some(texture.clone());
            }
            if frame == 30 {
                d.viewer.as_mut().unwrap().fullscreen = true;
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))),
                    ..Default::default()
                },
                |ui| { d.viewer_ui(ui.ctx(), false); },
            );
            output.textures_delta.clear();
            let window = d.viewer.as_ref().unwrap().float_rect.unwrap();
            if frame == 10 {
                first = window.size();
            }
            // The newly created fullscreen area needs its initial sizing pass.
            if frame > 10 && frame != 30 {
                assert!((window.size() - first).abs().max_elem() <= 1.0, "viewer resized from {first} to {}", window.size());
                let image = ctx.tessellate(output.shapes, output.pixels_per_point)
                    .iter()
                    .find_map(|shape| match &shape.primitive {
                        egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh.calc_bounds()),
                        _ => None
                    }).expect("VNC image");
                assert!((image.aspect_ratio() - 1.6).abs() < 0.01, "image is stretched");
                let viewer = d.viewer.as_ref().unwrap();
                assert_eq!((viewer.width, viewer.height), (1280, 800));
                if frame < 30 {
                    assert!((image.center().x - window.center().x).abs() <= 1.0, "image is not centered");
                    floating_image = image.size();
                } else if frame > 32 {
                    assert!(image.width() > floating_image.x && image.height() > floating_image.y);
                }
            }
            assert!(window.height() <= 680.0, "unexpected window size {window}");
        }
    }
    #[test]
    fn dragging_dock_divider_resizes_both_panes_within_their_limits() {
        let ctx = egui::Context::default();
        let mut desktops = Desktops::default();
        desktops.view(&json!({"id":"test"}), "wss://localhost:9", &ctx);
        let viewer = desktops.viewer.as_mut().unwrap();
        viewer.expanded = true;
        viewer.docked = true;
        let mut draw = |events| {
            let mut chat_width = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1900.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let dock_width = Desktops::dock_width(ui);
                    assert!(desktops.viewer_dock_ui(ui, dock_width));
                    chat_width = ui.available_width();
                },
            );
            output.textures_delta.clear();
            chat_width
        };

        draw(vec![]);
        let divider = egui::pos2(draw(vec![]), 450.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };

        draw(vec![egui::Event::PointerMoved(divider)]);
        draw(vec![button(divider, true)]);
        for (x, expected) in [(850.0, 850.0), (1800.0, 1540.0), (100.0, 340.0)] {
            let actual = draw(vec![egui::Event::PointerMoved(egui::pos2(x, divider.y))]);
            assert!(
                (actual - expected).abs() <= 2.0,
                "chat width {actual}, expected {expected}"
            );
        }
        draw(vec![button(egui::pos2(100.0, divider.y), false)]);
        assert!((draw(vec![]) - 340.0).abs() <= 2.0);
    }

    #[test]
    fn opted_in_desktop_docks_with_chat_space_and_falls_back_in_portrait() {
        let ctx = egui::Context::default();
        let mut d = Desktops::default();
        d.view(&json!({"id":"test","name":"Desktop"}), "wss://localhost:9", &ctx);
        let v = d.viewer.as_mut().unwrap();
        v.expanded = true;
        v.link = Some(workspace::connect(v.base.clone(), Some(v.id.clone()), ctx.clone()));
        let connection = v.link.as_ref().unwrap().frame.clone();
        let texture = ctx.load_texture(
            "docked desktop",
            egui::ColorImage::filled([2, 2], egui::Color32::BLACK),
            egui::TextureOptions::LINEAR,
        );
        v.texture = Some(texture.clone());
        for (width, height, opted_in, fullscreen, expected_dock) in [
            (1900.0, 900.0, false, false, false),
            (1900.0, 900.0, true, false, true),
            (1900.0, 900.0, true, true, false),
            (740.0, 1024.0, true, false, false),
            (1024.0, 740.0, true, false, true),
        ] {
            let v = d.viewer.as_mut().unwrap();
            v.docked = opted_in;
            v.fullscreen = fullscreen;
            let mut chat_width = 0.0;
            let mut image_rect = None;
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))),
                        ..Default::default()
                    },
                    |ui| {
                        let dock_width = Desktops::dock_width(ui);
                        let docked = d.viewer_dock_ui(ui, dock_width);
                        assert_eq!(docked, expected_dock, "screen {width} × {height}");
                        chat_width = ui.available_width();
                        if !docked {
                            d.viewer_ui(ui.ctx(), dock_width.is_some());
                        }
                    },
                );
                output.textures_delta.clear();
                image_rect = ctx.tessellate(output.shapes, output.pixels_per_point)
                    .iter()
                    .find_map(|shape| match &shape.primitive {
                        egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh.calc_bounds()),
                        _ => None,
                    });
            }
            let image = image_rect.expect("VNC image");
            assert!((image.aspect_ratio() - 1.6).abs() < 0.01, "{image}");
            if expected_dock {
                assert!(chat_width >= 340.0);
                assert!(chat_width < width / 2.0);
                assert!(image.left() >= chat_width);
                if width == 1900.0 {
                    assert!((chat_width - 475.0).abs() <= 2.0, "chat width {chat_width}");
                }
            } else {
                assert_eq!(chat_width, width);
            }
            let link = d.viewer.as_ref().unwrap().link.as_ref().unwrap();
            assert!(std::sync::Arc::ptr_eq(&link.frame, &connection));
        }
    }

    #[test]
    fn desktop_skeleton_fits_phone_and_desktop() {
        for width in [320.0,390.0,900.0] {
            let ctx=egui::Context::default();let mut used=egui::Rect::NOTHING;
            let mut output=ctx.run_ui(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(width,300.0))),..Default::default()},|ui|{
                desktop_skeleton(ui,1280,800);used=ui.min_rect();
            });output.textures_delta.clear();assert!(used.max.x<=width && used.max.y<=300.0);
        }
    }
    #[test]
    fn reopening_bound_chat_restores_bubble_without_video_connection() {
        let mut desktops = bound_chats();
        let ctx = egui::Context::default();
        desktops.follow_session("chat-a", "wss://example.test", &ctx);
        let v = desktops.viewer.as_ref().unwrap();
        assert_eq!(v.id, "desktop-a");
        assert!(!v.expanded);
        assert!(v.link.is_none());
        assert!(v.texture.is_none());
    }
    #[test]
    fn switching_chats_restores_their_own_bubble_and_position() {
        let mut desktops = bound_chats();
        let ctx = egui::Context::default();
        desktops.follow_session("chat-a", "wss://example.test", &ctx);
        desktops.viewer.as_mut().unwrap().bubble_pos = Some(egui::pos2(30.0, 40.0));
        desktops.follow_session("chat-b", "wss://example.test", &ctx);
        assert_eq!(desktops.viewer.as_ref().unwrap().id, "desktop-b");
        desktops.follow_session("unbound", "wss://example.test", &ctx);
        assert!(desktops.viewer.is_none());
        desktops.follow_session("chat-a", "wss://example.test", &ctx);
        assert_eq!(
            desktops.viewer.as_ref().unwrap().bubble_pos,
            Some(egui::pos2(30.0, 40.0))
        );
    }
    #[test]
    fn polling_same_binding_does_not_reset_open_viewer_or_dismissed_bubble() {
        let mut desktops = bound_chats();
        let ctx = egui::Context::default();
        desktops.follow_session("chat-a", "wss://example.test", &ctx);
        desktops.viewer.as_mut().unwrap().expanded = true;
        desktops.follow_session("chat-a", "wss://example.test", &ctx);
        assert!(desktops.viewer.as_ref().unwrap().expanded);
        desktops.viewer = None;
        desktops.follow_session("chat-a", "wss://example.test", &ctx);
        assert!(desktops.viewer.is_none());
    }
    #[test]
    fn scrolling_over_chat_stays_outside_the_zoomed_desktop_input() {
        let panned_image = egui::Rect::from_min_max(egui::pos2(200.0, 60.0), egui::pos2(900.0, 520.0));
        let visible = egui::Rect::from_min_max(egui::pos2(340.0, 60.0), egui::pos2(900.0, 520.0));
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 40.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        };
        let mut buttons = 0;
        let packets = pointer_packets(
            &[egui::Event::PointerMoved(egui::pos2(280.0, 100.0)), wheel.clone()],
            panned_image,
            visible,
            1.0,
            (700, 460),
            &mut buttons,
            Some(egui::pos2(400.0, 100.0)),
        );
        assert!(packets.is_empty());
        let packets = pointer_packets(
            &[wheel],
            panned_image,
            visible,
            1.0,
            (700, 460),
            &mut buttons,
            Some(egui::pos2(400.0, 100.0)),
        );
        assert_eq!(packets[0]["buttons"], 8);
        assert_eq!(packets[0]["x"], 200);
    }

    #[test]
    fn losing_focus_releases_held_mouse_buttons() {
        let mut buttons = 1;
        let out = pointer_packets(
            &[egui::Event::WindowFocused(false)],
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0)),
            egui::Rect::EVERYTHING,
            1.0,
            (100, 100),
            &mut buttons,
            Some(egui::pos2(20.0, 20.0)),
        );
        assert_eq!(buttons, 0);
        assert_eq!(out[0]["buttons"], 0);
    }

    #[test]
    fn click_press_and_release_in_same_frame_both_reach_vnc() {
        let pos = egui::pos2(60.0, 40.0);
        let events = [true, false].map(|pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        let mut buttons = 0;
        let out = pointer_packets(
            &events,
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 80.0)),
            egui::Rect::EVERYTHING,
            0.5,
            (200, 160),
            &mut buttons,
            None,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["buttons"], 1);
        assert_eq!(out[1]["buttons"], 0);
        assert_eq!(out[0]["x"], 100);
        assert_eq!(buttons, 0);
    }
}
