use crate::{gateway::codex::OrganizerCall, icon_text, preferences, session_tabs::TabId};
use eframe::egui;
use lucide_icons::Icon;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs, io,
    time::{Duration, Instant},
};
use uuid::Uuid;

const FILE: &str = "chat-organizer.json";
const AUTOSAVE_DELAY: Duration = Duration::from_millis(350);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(137, 216, 200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    ToDos,
    Notes,
}

impl Kind {
    fn untitled(self) -> &'static str {
        match self {
            Self::ToDos => "Untitled list",
            Self::Notes => "Untitled note",
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Library {
    items: Vec<Item>,
}

impl Library {
    fn load() -> io::Result<Self> {
        match fs::read(preferences::dir()?.join(FILE)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    fn create(&mut self, kind: Kind, chat: &TabId) -> Uuid {
        let id = Uuid::new_v4();
        self.items.push(Item {
            id,
            title: String::new(),
            chats: vec![chat.clone()],
            content: match kind {
                Kind::ToDos => Content::ToDos(Vec::new()),
                Kind::Notes => Content::Note(String::new()),
            },
        });
        id
    }

    fn tool(&mut self, chat: &TabId, operation: Uuid, args: &Value) -> Result<Value, String> {
        if tool_action(args) == "list" {
            let items: Vec<_> = self
                .items
                .iter()
                .filter(|item| item.chats.contains(chat))
                .map(Item::summary)
                .collect();
            return Ok(json!({"items": items}));
        }

        let action = tool_action(args);
        let creating = matches!(action, "create_note" | "create_checklist");
        let id = if creating {
            operation
        } else {
            tool_id(args, "id")?
        };
        if creating && !self.items.iter().any(|item| item.id == id) {
            let title = tool_text(args, "title", 512)?.to_owned();
            let content = if action == "create_note" {
                let text = if args.get("text").is_none() {
                    String::new()
                } else {
                    tool_text(args, "text", 65_536)?.to_owned()
                };
                Content::Note(text)
            } else {
                Content::ToDos(Vec::new())
            };
            self.items.push(Item {
                id,
                title,
                chats: vec![chat.clone()],
                content,
            });
        }

        let item = self
            .items
            .iter_mut()
            .find(|item| item.id == id && item.chats.contains(chat))
            .ok_or("Item not attached to this chat")?;
        match action {
            "create_note" | "create_checklist" => Ok(item.summary()),
            "read" => {
                let mut result = item.summary();
                match &item.content {
                    Content::Note(text) => result["text"] = json!(text),
                    Content::ToDos(rows) => result["rows"] = json!(rows),
                }
                Ok(result)
            }
            "rename" => {
                item.title = tool_text(args, "title", 512)?.to_owned();
                Ok(item.summary())
            }
            "update_note" => {
                let Content::Note(text) = &mut item.content else {
                    return Err("Item is not a note".into());
                };
                *text = tool_text(args, "text", 65_536)?.to_owned();
                Ok(item.summary())
            }
            "add_todo" | "edit_todo" | "set_done" => {
                let Content::ToDos(rows) = &mut item.content else {
                    return Err("Item is not a checklist".into());
                };
                let id = if action == "add_todo" {
                    operation
                } else {
                    tool_id(args, "rowId")?
                };
                if action == "add_todo" && !rows.iter().any(|row| row.id == id) {
                    rows.push(Row {
                        id,
                        text: todo_text(args)?,
                        done: false,
                    });
                }
                let row = rows
                    .iter_mut()
                    .find(|row| row.id == id)
                    .ok_or("To-do not found")?;
                match action {
                    "edit_todo" => row.text = todo_text(args)?,
                    "set_done" => {
                        row.done = args["done"].as_bool().ok_or("done must be a boolean")?
                    }
                    _ => {}
                }
                Ok(json!({"row": row}))
            }
            _ => Err("Unknown organizer action".into()),
        }
    }

    fn attached<'a>(&'a self, kind: Kind, chat: &'a TabId) -> impl Iterator<Item = &'a Item> {
        self.items
            .iter()
            .filter(move |item| item.kind() == kind && item.chats.contains(chat))
    }

    fn attach(&mut self, id: Uuid, chat: &TabId) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if item.chats.contains(chat) {
            return false;
        }
        item.chats.push(chat.clone());
        true
    }

    fn detach(&mut self, id: Uuid, chat: &TabId) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        let before = item.chats.len();
        item.chats.retain(|attached| attached != chat);
        before != item.chats.len()
    }
}

fn tool_action(args: &Value) -> &str {
    args["action"].as_str().unwrap_or_default()
}

fn tool_id(args: &Value, key: &str) -> Result<Uuid, String> {
    args[key]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| format!("{key} must be an item or row ID"))
}

fn tool_text<'a>(args: &'a Value, key: &str, max_bytes: usize) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .filter(|text| text.len() <= max_bytes)
        .ok_or_else(|| format!("{key} must be text of up to {max_bytes} bytes"))
}

fn todo_text(args: &Value) -> Result<String, String> {
    let text = tool_text(args, "text", 4096)?.trim();
    if text.is_empty() {
        return Err("To-do text cannot be blank".into());
    }
    Ok(text.to_owned())
}

#[derive(Clone, Deserialize, Serialize)]
struct Item {
    id: Uuid,
    title: String,
    chats: Vec<TabId>,
    content: Content,
}

impl Item {
    fn kind(&self) -> Kind {
        match self.content {
            Content::Note(_) => Kind::Notes,
            Content::ToDos(_) => Kind::ToDos,
        }
    }

    fn name(&self) -> &str {
        if self.title.trim().is_empty() {
            self.kind().untitled()
        } else {
            self.title.trim()
        }
    }

    fn summary(&self) -> Value {
        let kind = match self.kind() {
            Kind::Notes => "note",
            Kind::ToDos => "checklist",
        };
        json!({"id": self.id, "title": self.name(), "kind": kind})
    }

    fn for_message(&self) -> String {
        let body = match &self.content {
            Content::Note(text) => text.clone(),
            Content::ToDos(rows) => rows
                .iter()
                .map(|row| format!("- [{}] {}", if row.done { 'x' } else { ' ' }, row.text))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        format!("## {}\n\n{body}", self.name())
    }
}

#[derive(Clone, Deserialize, Serialize)]
enum Content {
    Note(String),
    ToDos(Vec<Row>),
}

#[derive(Clone, Deserialize, Serialize)]
struct Row {
    id: Uuid,
    text: String,
    done: bool,
}

impl Row {
    fn new(text: String, done: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            text,
            done,
        }
    }
}

#[derive(Default)]
pub(crate) struct Organizer {
    library: Option<Library>,
    error: Option<String>,
    pending_save: Option<Instant>,
    preview: Option<Uuid>,
    renaming: Option<Uuid>,
    markdown: egui_commonmark::CommonMarkCache,
}

impl Organizer {
    pub(crate) fn answer(&mut self, url: &str, call: OrganizerCall) -> Value {
        // Use the originating thread even when a different chat is visible.
        let chat = TabId::new(url, &call.thread);
        let (success, text) = match self.tool(&chat, call.operation, &call.args) {
            Ok(value) => (true, value.to_string()),
            Err(error) => (false, error),
        };
        json!({"token":call.token,"success":success,"text":text})
    }

    fn tool(&mut self, chat: &TabId, operation: Uuid, args: &Value) -> Result<Value, String> {
        if self.library.is_none() {
            self.library = Some(Library::load().map_err(|error| error.to_string())?);
        }
        let library = self.library.as_mut().unwrap();
        if matches!(tool_action(args), "list" | "read") {
            return library.tool(chat, operation, args);
        }

        // Commit before reporting success. Keep sidebar edits if saving fails.
        let mut edited = library.clone();
        let result = edited.tool(chat, operation, args)?;
        serde_json::to_value(&edited)
            .map_err(io::Error::from)
            .and_then(|value| preferences::write(FILE, &value))
            .map_err(|error| error.to_string())?;
        *library = edited;
        self.pending_save = None;
        self.error = None;
        Ok(result)
    }

    pub(crate) fn rekey(&mut self, old: &TabId, new: &TabId) {
        // Attachments follow a draft when the server assigns its canonical key.
        if self.library.is_none() {
            self.library = Library::load().ok();
        }
        let Some(library) = &mut self.library else {
            return;
        };
        let mut changed = false;
        for item in &mut library.items {
            if let Some(index) = item.chats.iter().position(|chat| chat == old) {
                if item.chats.contains(new) {
                    item.chats.remove(index);
                } else {
                    item.chats[index] = new.clone();
                }
                changed = true;
            }
        }
        if changed {
            self.pending_save = Some(Instant::now());
        }
    }

    pub(crate) fn flush(&mut self) {
        if self.pending_save.take().is_none() && self.error.is_none() {
            return;
        }
        let Some(library) = &self.library else { return };
        self.error = serde_json::to_value(library)
            .map_err(io::Error::from)
            .and_then(|value| preferences::write(FILE, &value))
            .err()
            .map(|error| error.to_string());
    }

    pub(crate) fn tick(&mut self, ctx: &egui::Context) {
        if let Some(deadline) = self.pending_save {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.flush();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }

    // This runs in the right-aligned workspace header, outside the scroll area.
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, kind: Kind, chat: &TabId) {
        if self.library.is_none() {
            match Library::load() {
                Ok(library) => {
                    self.library = Some(library);
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        let Some(library) = self.library.as_mut() else {
            return;
        };
        let (create_icon, create_label) = match kind {
            Kind::ToDos => (Icon::ListPlus, "New checklist"),
            Kind::Notes => (Icon::FilePlus, "New note"),
        };
        let attach = ui
            .add(crate::workspace_icon(Icon::Paperclip))
            .on_hover_text("Attach existing");
        let item_to_attach = egui::Popup::menu(&attach)
            .show(|ui| {
                ui.label(egui::RichText::new("Attach existing").strong());
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        ui.set_width(200.0);
                        let mut found = false;
                        for item in library
                            .items
                            .iter()
                            .filter(|item| item.kind() == kind && !item.chats.contains(chat))
                        {
                            found = true;
                            if ui.add(egui::Button::new(item.name()).truncate()).clicked() {
                                ui.close();
                                return Some(item.id);
                            }
                        }
                        if !found {
                            ui.weak("Nothing to attach");
                        }
                        None
                    })
                    .inner
            })
            .and_then(|popup| popup.inner);
        let mut changed = item_to_attach.is_some_and(|id| library.attach(id, chat));
        if ui
            .add(
                egui::Button::new(icon_text(create_icon, 16.0).color(ACCENT))
                    .min_size(egui::vec2(28.0, 28.0))
                    .fill(egui::Color32::from_rgb(36, 65, 61)),
            )
            .on_hover_text(create_label)
            .clicked()
        {
            self.renaming = Some(library.create(kind, chat));
            changed = true;
        }
        if changed {
            self.schedule_save(ui.ctx());
        }
    }

    fn schedule_save(&mut self, ctx: &egui::Context) {
        self.pending_save = Some(Instant::now() + AUTOSAVE_DELAY);
        ctx.request_repaint_after(AUTOSAVE_DELAY);
    }

    pub(crate) fn ui(&mut self, ui: &mut egui::Ui, kind: Kind, chat: &TabId) -> Option<String> {
        if self.library.is_none() {
            if let Some(error) = &self.error {
                ui.label(format!("Could not open To-Dos and Notes: {error}"));
            }
            return None;
        }
        if let Some(error) = &self.error {
            ui.label(format!("Changes not saved: {error}"));
            if ui.small_button("Retry save").clicked() {
                self.pending_save = Some(Instant::now());
                self.flush();
            }
        }
        let library = self.library.as_mut()?;
        let empty_label = match kind {
            Kind::ToDos => "No checklists in this chat",
            Kind::Notes => "No notes in this chat",
        };
        let mut changed = false;
        if library.attached(kind, chat).next().is_none() {
            egui::Frame::group(ui.style())
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(egui::RichText::new(empty_label).strong());
                    ui.weak("Create one or attach one you already have.");
                });
        }

        let mut detach = None;
        let mut message = None;
        for item in library
            .items
            .iter_mut()
            .filter(|item| item.kind() == kind && item.chats.contains(chat))
        {
            ui.push_id(item.id, |ui| {
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgb(33, 35, 41))
                    .inner_margin(8)
                    .corner_radius(10)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        egui::CollapsingHeader::new(egui::RichText::new(item.name()).strong())
                            .id_salt("item")
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.add_space(3.0);
                                if self.renaming == Some(item.id) {
                                    let title = ui.add(
                                        egui::TextEdit::singleline(&mut item.title)
                                            .hint_text("Title")
                                            .desired_width(f32::INFINITY),
                                    );
                                    changed |= title.changed();
                                    if title.lost_focus() {
                                        self.renaming = None;
                                    } else if !title.has_focus() {
                                        title.request_focus();
                                    }
                                }
                                match &mut item.content {
                                    Content::Note(body) => {
                                        ui.horizontal(|ui| {
                                            let preview = self.preview == Some(item.id);
                                            if ui.selectable_label(!preview, "Write").clicked() {
                                                self.preview = None;
                                            }
                                            if ui.selectable_label(preview, "Preview").clicked() {
                                                self.preview = Some(item.id);
                                            }
                                        });
                                        if self.preview == Some(item.id) {
                                            if body.trim().is_empty() {
                                                ui.weak("Nothing to preview yet");
                                            } else {
                                                egui_commonmark::CommonMarkViewer::new().show(
                                                    ui,
                                                    &mut self.markdown,
                                                    body,
                                                );
                                            }
                                        } else {
                                            changed |= ui
                                                .add(
                                                    egui::TextEdit::multiline(body)
                                                        .desired_rows(4)
                                                        .desired_width(f32::INFINITY)
                                                        .hint_text("Write a note in Markdown…"),
                                                )
                                                .changed();
                                        }
                                    }
                                    Content::ToDos(rows) => changed |= checklist(ui, rows),
                                }
                                ui.add_space(3.0);
                                ui.separator();
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .add(crate::workspace_icon(Icon::ClipboardPaste))
                                        .on_hover_text("Add to draft without sending")
                                        .clicked()
                                    {
                                        message = Some(item.for_message());
                                    }
                                    ui.menu_button(icon_text(Icon::Ellipsis, 16.0), |ui| {
                                        if ui.button("Rename").clicked() {
                                            self.renaming = Some(item.id);
                                            ui.close();
                                        }
                                        if ui
                                            .button("Detach from chat")
                                            .on_hover_text(
                                                "The item stays available to attach again",
                                            )
                                            .clicked()
                                        {
                                            detach = Some(item.id);
                                            ui.close();
                                        }
                                    })
                                    .response
                                    .on_hover_text("More actions");
                                    if item.chats.len() > 1 {
                                        ui.weak(format!("Shared · {} chats", item.chats.len()));
                                    }
                                });
                            });
                    });
            });
            ui.add_space(6.0);
        }
        if let Some(id) = detach {
            changed |= library.detach(id, chat);
        }
        if changed {
            self.schedule_save(ui.ctx());
        }
        message
    }
}

fn checklist(ui: &mut egui::Ui, rows: &mut Vec<Row>) -> bool {
    if !rows.is_empty() {
        let done = rows.iter().filter(|row| row.done).count();
        ui.weak(format!("{done} of {} done", rows.len()));
        ui.add(
            egui::ProgressBar::new(done as f32 / rows.len() as f32)
                .fill(ACCENT)
                .desired_height(4.0)
                .corner_radius(2),
        );
        ui.add_space(3.0);
    }
    let mut changed = false;
    let mut remove = None;
    for row in rows.iter_mut() {
        ui.push_id(row.id, |ui| {
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut row.done, "")
                    .on_hover_text("Mark complete")
                    .changed();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.small_button("×").on_hover_text("Remove to-do").clicked() {
                            remove = Some(row.id);
                        }
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut row.text)
                                    .desired_width(f32::INFINITY)
                                    .horizontal_align(egui::Align::LEFT)
                                    .hint_text("To-do"),
                            )
                            .changed();
                    },
                );
            });
        });
    }
    if let Some(id) = remove {
        rows.retain(|row| row.id != id);
        changed = true;
    }
    if ui.button("+ Add to-do").clicked() {
        let row = Row::new(String::new(), false);
        rows.push(row);
        changed = true;
    }
    changed
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_updates_are_scoped_and_retried_creates_do_not_duplicate() {
        let chat = TabId::new("wss://one.example", "thread");
        let other = TabId::new("wss://two.example", "thread");
        let mut library = Library::default();
        let hidden = library.create(Kind::Notes, &other);
        let list = Uuid::new_v4();
        let create = serde_json::json!({"action":"create_checklist","title":"Release"});
        for _ in 0..2 {
            library.tool(&chat, list, &create).unwrap();
        }
        let row = Uuid::new_v4();
        let add = serde_json::json!({"action":"add_todo","id":list,"text":"Test the build"});
        for _ in 0..2 {
            library.tool(&chat, row, &add).unwrap();
        }
        library
            .tool(
                &chat,
                row,
                &serde_json::json!({
                    "action":"set_done","id":list,"rowId":row,"done":true
                }),
            )
            .unwrap();
        assert!(
            library
                .tool(
                    &chat,
                    row,
                    &serde_json::json!({
                        "action":"update_note","id":hidden,"text":"wrong server"
                    })
                )
                .is_err()
        );
        let result = library
            .tool(&chat, row, &serde_json::json!({"action":"list"}))
            .unwrap();
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["items"][0]["id"], list.to_string());
        assert!(result["items"][0].get("rows").is_none());
        let result = library
            .tool(&chat, row, &json!({"action":"read","id":list}))
            .unwrap();
        assert!(result.get("chats").is_none());
        assert_eq!(result["rows"].as_array().unwrap().len(), 1);
        assert_eq!(result["rows"][0]["done"], true);
    }

    #[test]
    fn edits_are_shared_and_detach_keeps_the_item() {
        let doom = TabId::new("wss://doom.example", "chat");
        let ssh = TabId::new("ssh://user@localhost:22", "chat");
        let mut library = Library::default();
        let id = library.create(Kind::Notes, &doom);
        assert!(library.attached(Kind::Notes, &ssh).next().is_none());
        assert!(library.attach(id, &ssh));
        assert!(!library.attach(id, &ssh));
        let note = library.items.iter_mut().find(|item| item.id == id).unwrap();
        note.content = Content::Note("**shared**".into());
        assert!(library.detach(id, &doom));
        assert!(library.attached(Kind::Notes, &doom).next().is_none());
        let saved = serde_json::to_vec(&library).unwrap();
        let mut restored: Library = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            restored
                .attached(Kind::Notes, &ssh)
                .next()
                .unwrap()
                .for_message(),
            "## Untitled note\n\n**shared**"
        );
        assert!(restored.detach(id, &ssh));
        assert!(restored.attach(id, &doom));
        assert_eq!(restored.items.len(), 1);
    }

    #[test]
    fn draft_attachments_follow_promotion_and_survive_reload() {
        let old = TabId::new("wss://doom.example", "draft:one");
        let new = TabId::new(&old.url, "thread");
        let mut library = Library::default();
        let id = library.create(Kind::Notes, &old);
        assert!(library.attach(id, &new));
        let mut organizer = Organizer {
            library: Some(library),
            ..Organizer::default()
        };
        organizer.rekey(&old, &new);
        organizer.flush();
        assert!(organizer.error.is_none());
        let saved = Library::load().unwrap();
        assert!(saved.attached(Kind::Notes, &old).next().is_none());
        assert_eq!(saved.items[0].chats, vec![new]);
    }

    #[test]
    fn checklist_status_survives_saving_and_is_explicit_in_message() {
        let chat = TabId::new("ssh://user@localhost:22", "chat");
        let mut library = Library::default();
        library.create(Kind::ToDos, &chat);
        library.items[0].content = Content::ToDos(vec![
            Row::new("Ready".into(), true),
            Row::new("Next".into(), false),
        ]);
        let saved = serde_json::to_vec(&library).unwrap();
        let restored: Library = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            restored
                .attached(Kind::ToDos, &chat)
                .next()
                .unwrap()
                .for_message(),
            "## Untitled list\n\n- [x] Ready\n- [ ] Next"
        );
    }
}
