use super::{D4c, activity, egui, gateway, json, preferences};
use shadcn_egui::{
    Theme,
    components::{
        button::{Button, ButtonSize, ButtonVariant},
        card::{Card, CardSize},
        dialog::Dialog,
        input::Input,
        switch::Switch,
    },
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ComposerPicker {
    Permissions,
    Model,
    Effort,
}

fn neutral(label: impl Into<String>) -> Button {
    Button::new(label).variant(ButtonVariant::Outline)
}

fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> bool {
    ui.horizontal(|ui| {
        let response = ui.add(Switch::new(value));
        let title = ui.label(label);
        response.labelled_by(title.id).changed()
    })
    .inner
}

fn surface(
    ctx: &egui::Context,
    compact: bool,
    id: &str,
    title: &str,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let bounds = crate::platform::content_rect(ctx);
    let max_height = (bounds.height() - 104.0).max(48.0);
    let content = |ui: &mut egui::Ui| {
        ui.scope(|body_ui| {
            body_ui.set_max_height(max_height);
            egui::ScrollArea::vertical()
                .id_salt(id)
                .max_height(max_height)
                .show(body_ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    body(ui);
                });
        });
    };
    if !compact {
        let mut open = true;
        Dialog::new(id, title)
            .width(400.0)
            .show_close_button(false)
            .show(ctx, &mut open, content);
        return !open;
    }
    let id = egui::Id::unique(id);
    let theme = Theme::current(ctx);
    let area = egui::Modal::default_area(id)
        .constrain_to(bounds)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::ZERO);
    egui::Modal::new(id)
        .area(area)
        .frame(
            egui::Frame::new()
                .fill(theme.palette().popover)
                .corner_radius(egui::CornerRadius {
                    nw: 16,
                    ne: 16,
                    sw: 0,
                    se: 0,
                })
                .inner_margin(16),
        )
        .show(ctx, |ui| {
            ui.set_width((bounds.width() - 32.0).max(1.0));
            ui.heading(title);
            ui.add_space(8.0);
            content(ui);
        })
        .should_close()
}

impl D4c {
    pub(super) fn dialog_open(&self) -> bool {
        self.composer_picker.is_some()
            || self.rename.is_some()
            || self.delete_chat.is_some()
            || self.switch_pending.is_some()
    }
    pub(super) fn dismiss_dialog(&mut self) -> bool {
        if self.rename.is_some() {
            if self.rename_pending.is_none() {
                self.rename = None;
            }
        } else if self.delete_chat.is_some() {
            if self.delete_pending.is_none() {
                self.delete_chat = None;
            }
        } else if self.switch_pending.take().is_none() && self.composer_picker.take().is_none() {
            return false;
        }
        true
    }
    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut switch = None;
        let mut forget = None;
        egui::ScrollArea::vertical()
            .id_salt("settings")
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(640.0));
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.heading("Settings");
                ui.add_space(8.0);
                ui.strong("Connections");
                for (index, connection) in self.preferences.connections.iter().enumerate() {
                    Card::new().size(CardSize::Sm).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&connection.name);
                            if index == self.preferences.selected {
                                ui.weak(if self.connected { "Online" } else { "Offline" });
                            } else {
                                if ui.add(neutral("Switch")).clicked() {
                                    switch = Some(index);
                                }
                                ui.menu_button("More", |ui| {
                                    if ui.add(neutral("Remove connection")).clicked() {
                                        forget = Some(index);
                                        ui.close();
                                    }
                                });
                            }
                        });
                        ui.add(
                            egui::Label::new(egui::RichText::new(&connection.url).small().weak())
                                .wrap(),
                        );
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(neutral(if self.connected {
                            "Reconnect"
                        } else {
                            "Connect"
                        }))
                        .clicked()
                    {
                        self.reconnect(ui.ctx());
                    }
                    if ui
                        .add_enabled(self.can_reload(), neutral("Reload current session"))
                        .on_hover_text("Refresh Codex tools and skills while the chat is idle")
                        .clicked()
                    {
                        self.reload_session();
                    }
                    if self.gateway.is_some() && ui.add(neutral("Disconnect")).clicked() {
                        self.gateway = None;
                        self.connected = false;
                        self.running = false;
                        self.active_sessions.clear();
                        self.live.clear();
                        self.status = "Disconnected".into();
                        self.cron.disconnected();
                    }
                });
                ui.add(
                    egui::Label::new(egui::RichText::new(&self.status).small().weak())
                        .wrap()
                        .selectable(true),
                );
                ui.weak("New devices need approval on the D4C server.");
                if preferences::valid_url(&self.url) {
                    egui::CollapsingHeader::new("Workspace server").show(ui, |ui| {
                        let value = self
                            .preferences
                            .workspace_urls
                            .entry(self.url.clone())
                            .or_default();
                        ui.add(
                            Input::new(value)
                                .hint("wss://workspace-host:8443 (optional)")
                                .max_width(f32::INFINITY),
                        );
                        if ui.add(neutral("Save workspace URL")).clicked() {
                            if value.is_empty() || preferences::valid_url(value) {
                                match self.preferences.save() {
                                    Ok(()) => {
                                        self.settings_error.clear();
                                        self.desktops.connect(&self.url, ui.ctx());
                                    }
                                    Err(error) => self.settings_error = error.to_string(),
                                }
                            } else {
                                self.settings_error =
                                    "Use a secure wss:// URL without credentials or query tokens"
                                        .into();
                            }
                        }
                    });
                }
                egui::CollapsingHeader::new("Add server")
                    .id_salt("add_server")
                    .show(ui, |ui| {
                        ui.add(
                            Input::new(&mut self.connection_name)
                                .hint("Name")
                                .max_width(f32::INFINITY),
                        );
                        ui.add(
                            Input::new(&mut self.connection_url)
                                .hint("wss://your-server")
                                .max_width(f32::INFINITY),
                        );
                        if ui.add(Button::new("Save connection")).clicked() {
                            self.save_connection();
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.strong("Notifications");
                let desktop_changed = toggle(ui, &mut self.preferences.desktop, "Desktop alerts");
                let sound_changed = toggle(ui, &mut self.preferences.sound, "Sound");
                if (desktop_changed || sound_changed)
                    && let Err(error) = self.preferences.save()
                {
                    self.settings_error = error.to_string();
                }
                ui.weak("Alerts appear on the device running D4C, while the app is connected.");
                if ui.add(neutral("Test notification")).clicked() {
                    let now = activity::now();
                    if self.center.push(json!({
                        "id": format!("test:{}", now / 1000), "title": "D4C",
                        "body": "Notifications are ready", "session": "", "time": now,
                        "kind": "test", "unread": true
                    })) {
                        self.notifier
                            .test(self.preferences.desktop, self.preferences.sound);
                    }
                }
                ui.weak(&self.notifier.status);
                for error in [&self.settings_error, &self.center.error] {
                    if !error.is_empty() {
                        ui.colored_label(Theme::current(ui.ctx()).palette().destructive, error);
                    }
                }
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
    fn save_connection(&mut self) {
        let url = self.connection_url.trim();
        let name = self.connection_name.trim();
        self.settings_error = if !preferences::valid_url(url) {
            "Use a secure wss:// address without credentials or query tokens".into()
        } else if name.is_empty() {
            "Give this connection a name".into()
        } else if self.preferences.connections.len() >= 12 {
            "Up to 12 connections are supported".into()
        } else if self.preferences.connections.iter().any(|c| c.url == url) {
            "This server is already saved".into()
        } else {
            self.preferences.connections.push(preferences::Connection {
                name: name.chars().take(60).collect(),
                url: url.into(),
            });
            match self.preferences.save() {
                Ok(()) => {
                    self.connection_name.clear();
                    self.connection_url.clear();
                    String::new()
                }
                Err(error) => {
                    self.preferences.connections.pop();
                    error.to_string()
                }
            }
        };
    }
    pub(super) fn dialogs_ui(&mut self, ctx: &egui::Context, compact: bool) {
        self.picker_ui(ctx, compact);
        if let Some(index) = self.switch_pending {
            let mut accept = false;
            let mut cancel = false;
            let dismissed = surface(ctx, compact, "switch_server", "Switch server?", |ui| {
                ui.label("Work is still running on this server. It will continue there while you switch.");
                ui.horizontal_wrapped(|ui| {
                    cancel = ui.add(neutral("Stay here")).clicked();
                    accept = ui.add(Button::new("Switch server")).clicked();
                });
            });
            if cancel || dismissed {
                self.switch_pending = None;
            }
            if accept {
                self.switch_pending = None;
                self.switch_connection(index, ctx);
            }
        }
        if let Some((key, title)) = self.delete_chat.clone() {
            let mut cancel = false;
            let mut confirm = false;
            let dismissed = surface(ctx, compact, "delete_chat", "Delete chat?", |ui| {
                ui.label(&title);
                ui.label("This removes the chat and its transcript.");
                if !self.delete_error.is_empty() {
                    ui.colored_label(
                        Theme::current(ctx).palette().destructive,
                        &self.delete_error,
                    );
                }
                ui.horizontal(|ui| {
                    cancel = ui
                        .add_enabled(self.delete_pending.is_none(), neutral("Cancel"))
                        .clicked();
                    confirm = ui
                        .add_enabled(
                            self.connected
                                && self.delete_pending.is_none()
                                && !self.active_sessions.contains(&key),
                            Button::new(if self.delete_pending.is_some() {
                                "Deleting…"
                            } else {
                                "Delete"
                            })
                            .variant(ButtonVariant::Destructive),
                        )
                        .clicked();
                });
            });
            if cancel || (dismissed && self.delete_pending.is_none()) {
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
            let dismissed = surface(ctx, compact, "rename_chat", "Rename chat", |ui| {
                ui.label("Chat name");
                let input = ui.add(
                    Input::new(&mut name)
                        .max_width(f32::INFINITY)
                        .id_salt("chat_name"),
                );
                if !self.rename_error.is_empty() {
                    ui.colored_label(
                        Theme::current(ctx).palette().destructive,
                        &self.rename_error,
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    cancel = ui
                        .add_enabled(self.rename_pending.is_none(), neutral("Cancel"))
                        .clicked();
                    save = ui
                        .add_enabled(
                            self.rename_pending.is_none() && !name.trim().is_empty(),
                            Button::new("Save"),
                        )
                        .clicked()
                        || (input.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && !name.trim().is_empty()
                            && self.rename_pending.is_none());
                });
            });
            if cancel || (dismissed && self.rename_pending.is_none()) {
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
                            self.rename_error = "Codex disconnected".into();
                        }
                    } else {
                        self.rename_error = "Connect to rename chats".into();
                    }
                }
            }
        }
    }
}

impl D4c {
    fn picker_trigger(&mut self, kind: ComposerPicker, response: egui::Response) {
        let active = self
            .composer_picker
            .as_ref()
            .is_some_and(|(open, _)| *open == kind);
        if response.clicked() && active {
            self.composer_picker = None;
        } else if active || response.clicked() {
            self.composer_picker = Some((kind, response));
        }
    }

    pub(super) fn permissions_trigger(&mut self, ui: &mut egui::Ui) {
        if self.permission_request.is_some() {
            ui.weak("Updating permissions…");
            return;
        }
        let caption = self.permission_modes.get(&self.session).map_or_else(
            || "Permissions".into(),
            |label| format!("Permissions · {label}"),
        );
        let trigger = ui
            .add_enabled(
                self.connected && !self.running && !self.history_loading,
                Button::new(caption.clone())
                    .icon(shadcn_egui::icons::lucide::SHIELD)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm),
            )
            .on_hover_text(caption);
        self.picker_trigger(ComposerPicker::Permissions, trigger);
    }

    pub(super) fn model_triggers(&mut self, ui: &mut egui::Ui) {
        let model = self
            .model
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Model");
        // Split the remaining toolbar space on narrow screens so a long model
        // name cannot displace the reasoning selector or the send action.
        let available = ui.available_width();
        let narrow = available < 260.0;
        let trigger = if narrow {
            ui.add_enabled_ui(!self.models.is_empty(), |ui| {
                ui.add_sized(
                    egui::vec2((available * 0.59 - 4.0).max(1.0), 30.0),
                    egui::Button::new(model).truncate(),
                )
            })
            .inner
            .on_hover_text(model)
        } else {
            ui.add_enabled(
                !self.models.is_empty(),
                neutral(model)
                    .size(ButtonSize::Sm)
                    .icon_end(shadcn_egui::icons::lucide::CHEVRON_DOWN),
            )
            .on_hover_text("Model")
        };
        self.picker_trigger(ComposerPicker::Model, trigger);
        let has_efforts = self
            .model_efforts
            .get(&self.model)
            .is_some_and(|(levels, _)| !levels.is_empty());
        let effort = if self.thinking.is_empty() {
            "Reasoning"
        } else {
            &self.thinking
        };
        let trigger = if narrow {
            ui.add_enabled_ui(has_efforts, |ui| {
                ui.add_sized(
                    egui::vec2((available * 0.41).max(1.0), 30.0),
                    egui::Button::new(effort).frame(false).truncate(),
                )
            })
            .inner
            .on_hover_text("Reasoning effort")
        } else {
            ui.add_enabled(
                has_efforts,
                Button::new(effort)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon_end(shadcn_egui::icons::lucide::CHEVRON_DOWN),
            )
            .on_hover_text("Reasoning effort")
        };
        self.picker_trigger(ComposerPicker::Effort, trigger);
    }

    fn picker_ui(&mut self, ctx: &egui::Context, compact: bool) {
        let Some((picker, trigger)) = self.composer_picker.clone() else {
            return;
        };
        let title = match picker {
            ComposerPicker::Permissions => "Permissions",
            ComposerPicker::Model => "Model",
            ComposerPicker::Effort => "Reasoning effort",
        };
        let mut selection = None;
        let mut options = |ui: &mut egui::Ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut option = |key: &str, label: &str, selected: bool, enabled: bool| {
                let row = ui
                    .add_enabled_ui(enabled, |ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), if compact { 42.0 } else { 32.0 }),
                            egui::Sense::click(),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::Button,
                                enabled,
                                selected,
                                label,
                            )
                        });
                        let colors = Theme::current(ui.ctx());
                        let colors = colors.palette();
                        if selected || response.hovered() || response.has_focus() {
                            ui.painter().rect_filled(rect, 6, colors.accent);
                        }
                        let text_rect = egui::Rect::from_min_max(
                            rect.min + egui::vec2(10.0, 0.0),
                            rect.max - egui::vec2(32.0, 0.0),
                        );
                        super::paint_ellipsis(ui, text_rect, label, 14.0, colors.foreground);
                        if selected {
                            ui.painter().text(
                                rect.right_center() - egui::vec2(16.0, 0.0),
                                egui::Align2::CENTER_CENTER,
                                char::from(lucide_icons::Icon::Check),
                                egui::FontId::new(15.0, egui::FontFamily::Name("lucide".into())),
                                colors.foreground,
                            );
                        }
                        response
                    })
                    .inner;
                if row.clicked() {
                    selection = Some(key.to_owned());
                }
            };
            match picker {
                ComposerPicker::Permissions => {
                    let current = self.permission_modes.get(&self.session).map(String::as_str);
                    for (mode, label) in [
                        ("default", "Default"),
                        ("read-only", "Read only"),
                        ("yolo", "Full access (YOLO)"),
                    ] {
                        option(
                            mode,
                            label,
                            current == Some(label),
                            self.connected
                                && !self.running
                                && !self.history_loading
                                && self.permission_request.is_none(),
                        );
                    }
                }
                ComposerPicker::Model => {
                    for model in &self.models {
                        option(model, model, *model == self.model, true);
                    }
                }
                ComposerPicker::Effort => {
                    if let Some((levels, _)) = self.model_efforts.get(&self.model) {
                        for level in levels {
                            option(level, level, *level == self.thinking, true);
                        }
                    }
                }
            }
        };
        let dismissed = if compact {
            surface(ctx, true, title, title, options)
        } else {
            let mut open = true;
            let palette = Theme::current(ctx);
            let palette = palette.palette();
            egui::Popup::from_response(&trigger)
                .id(trigger.id.with("choices"))
                .align(egui::RectAlign::TOP_START)
                .gap(5.0)
                .width(220.0)
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .frame(
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0, palette.border))
                        .fill(palette.popover)
                        .inner_margin(5)
                        .corner_radius(9),
                )
                .show(|ui| {
                    ui.set_width(210.0);
                    egui::ScrollArea::vertical()
                        .id_salt(title)
                        .max_height(320.0)
                        .show(ui, |ui| options(ui));
                });
            !open
        };
        if dismissed || selection.is_some() {
            self.composer_picker = None;
        }
        let Some(value) = selection else { return };
        match picker {
            ComposerPicker::Permissions => self.set_permissions(&value),
            ComposerPicker::Model if self.model != value => {
                self.model = value;
                self.normalize_effort();
                self.remember_selection();
                self.patch_selection();
            }
            ComposerPicker::Effort if self.thinking != value => {
                self.thinking = value;
                self.remember_selection();
                self.patch_selection();
            }
            _ => {}
        }
    }
}
