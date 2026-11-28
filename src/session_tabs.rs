use eframe::egui::{
    self, Align, Color32, CornerRadius, PointerButton, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};
use serde::{Deserialize, Serialize};

const LAYOUT_FILE: &str = "session-tabs.json";
const ACCENT: Color32 = Color32::from_rgb(132, 203, 189);
const NUMBER_KEYS: [egui::Key; 9] = [
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct TabId {
    pub url: String,
    pub session: String,
}

impl TabId {
    pub fn new(url: &str, session: &str) -> Self {
        Self {
            url: url.into(),
            session: session.into(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Tab {
    id: TabId,
    title: String,
}

#[derive(Serialize, Deserialize)]
struct Layout {
    tabs: Vec<Tab>,
    active: TabId,
}

#[derive(Default)]
pub(crate) struct Tabs {
    entries: Vec<Tab>,
    visible_selection: Option<TabId>,
    saved: Option<Layout>,
}

pub(crate) enum Action {
    Select(TabId),
    Close(TabId),
}

impl Tabs {
    pub fn load(known_server: impl Fn(&str) -> bool) -> Option<(Self, Option<TabId>)> {
        Self::restore(crate::preferences::read(LAYOUT_FILE), known_server)
    }

    fn restore(
        value: serde_json::Value,
        known_server: impl Fn(&str) -> bool,
    ) -> Option<(Self, Option<TabId>)> {
        let saved: Layout = serde_json::from_value(value).ok()?;
        let mut seen = std::collections::HashSet::new();
        let entries: Vec<_> = saved
            .tabs
            .iter()
            .filter(|tab| known_server(&tab.id.url) && seen.insert(&tab.id))
            .cloned()
            .collect();
        let active = if known_server(&saved.active.url) {
            Some(saved.active.clone())
        } else {
            entries.first().map(|tab| tab.id.clone())
        };
        Some((
            Self {
                entries,
                saved: Some(saved),
                ..Self::default()
            },
            active,
        ))
    }

    fn layout(&self, active: &TabId) -> Layout {
        Layout {
            tabs: self.entries.clone(),
            active: active.clone(),
        }
    }

    pub fn save(&mut self, active: &TabId) -> std::io::Result<()> {
        if self
            .saved
            .as_ref()
            .is_some_and(|saved| &saved.active == active && saved.tabs == self.entries)
        {
            return Ok(());
        }
        let layout = self.layout(active);
        crate::preferences::write(LAYOUT_FILE, &serde_json::to_value(&layout)?)?;
        self.saved = Some(layout);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn consume_number_shortcut(&self, input: &mut egui::InputState) -> Option<TabId> {
        let mut selected = None;
        let mut suppress_next_text = false;
        input.events.retain(|event| {
            // egui-winit can emit a Text event immediately after an Alt keypress.
            if std::mem::take(&mut suppress_next_text) && matches!(event, egui::Event::Text(_)) {
                return false;
            }
            let egui::Event::Key {
                key,
                pressed: true,
                repeat,
                modifiers: egui::Modifiers::ALT,
                ..
            } = event
            else {
                return true;
            };
            let Some(index) = NUMBER_KEYS.iter().position(|number| number == key) else {
                return true;
            };
            suppress_next_text = true;
            if !repeat && let Some(tab) = self.entries.get(index) {
                selected = Some(tab.id.clone());
            }
            false
        });
        selected
    }

    #[cfg(test)]
    pub fn ids(&self) -> impl Iterator<Item = &TabId> {
        self.entries.iter().map(|tab| &tab.id)
    }

    pub fn open(&mut self, id: TabId) {
        if self.position(&id).is_none() {
            self.entries.push(Tab {
                id,
                title: "New chat".into(),
            });
        }
    }

    fn position(&self, id: &TabId) -> Option<usize> {
        self.entries.iter().position(|tab| &tab.id == id)
    }

    pub fn neighbor(&self, id: &TabId) -> Option<&TabId> {
        let index = self.position(id)?;
        self.entries
            .get(index + 1)
            .or_else(|| index.checked_sub(1).and_then(|left| self.entries.get(left)))
            .map(|tab| &tab.id)
    }

    pub fn remove(&mut self, id: &TabId) {
        if let Some(index) = self.position(id) {
            self.entries.remove(index);
        }
    }

    pub fn rekey(&mut self, url: &str, old: &str, new: &str) {
        let old = TabId::new(url, old);
        let new = TabId::new(url, new);
        if self.position(&old).is_some() {
            self.remove(&new);
            if let Some(index) = self.position(&old) {
                self.entries[index].id = new;
            }
        }
    }

    fn move_to_gap(&mut self, from: usize, gap: usize) {
        let to = if gap > from { gap - 1 } else { gap };
        if to != from {
            let tab = self.entries.remove(from);
            self.entries.insert(to, tab);
        }
    }

    pub fn ui<'a>(
        &mut self,
        ui: &mut egui::Ui,
        active: &TabId,
        mut labels: impl FnMut(&TabId) -> (Option<&'a str>, Option<&'a str>),
    ) -> Option<Action> {
        let reveal = self.visible_selection.as_ref() != Some(active);
        if reveal {
            self.visible_selection = Some(active.clone());
        }
        let width = ui.available_width();
        let scroll = egui::ScrollArea::horizontal()
            .id_salt("session-tabs")
            .max_width(width)
            .min_scrolled_width(0.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let viewport = ui.clip_rect();
                let mut rects = Vec::with_capacity(self.entries.len());
                let mut action = None;
                let mut drag = None;
                let count = self.entries.len();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, tab) in self.entries.iter_mut().enumerate() {
                        let (title, server) = labels(&tab.id);
                        if let Some(title) = title
                            && tab.title != title
                        {
                            tab.title = title.into();
                        }
                        let selected = &tab.id == active;
                        let font = egui::FontId::proportional(13.0);
                        let text_width = ui.fonts_mut(|fonts| {
                            fonts
                                .layout_no_wrap(tab.title.clone(), font, Color32::WHITE)
                                .size()
                                .x
                        });
                        let tab_width = (text_width + 44.0).clamp(84.0, 190.0).min(width.max(44.0));
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(tab_width, 30.0), Sense::hover());
                        rects.push(rect);
                        let close_rect =
                            Rect::from_min_max(pos2(rect.right() - 26.0, rect.top()), rect.max);
                        let body = ui.interact(
                            Rect::from_min_max(rect.min, pos2(close_rect.left(), rect.bottom())),
                            ui.scope_id().with((&tab.id, "select")),
                            Sense::click_and_drag(),
                        );
                        let close = ui.interact(
                            close_rect,
                            ui.scope_id().with((&tab.id, "close")),
                            Sense::click(),
                        );
                        let dragging = body.dragged_by(PointerButton::Primary);
                        if dragging || body.drag_stopped_by(PointerButton::Primary) {
                            drag = Some((index, dragging));
                        }
                        if selected && reveal && !viewport.contains_rect(rect) {
                            ui.scroll_to_rect(rect, Some(Align::Center));
                        }
                        let painter = ui.painter();
                        let left = if index == 0 { 6 } else { 0 };
                        let right = if index + 1 == count { 6 } else { 0 };
                        let corners = CornerRadius {
                            nw: left,
                            ne: right,
                            sw: left,
                            se: right,
                        };
                        let fill = if selected {
                            Color32::from_rgb(54, 64, 67)
                        } else if body.hovered() || close.hovered() || dragging {
                            Color32::from_rgb(45, 48, 54)
                        } else {
                            Color32::from_rgb(35, 37, 42)
                        };
                        let border = Stroke::new(1.0, Color32::from_rgb(63, 67, 73));
                        painter.rect(rect, corners, fill, border, StrokeKind::Middle);
                        if selected {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    rect.left_top() + vec2(5.0, 1.0),
                                    rect.right_top() + vec2(-5.0, 3.0),
                                ),
                                1.0,
                                ACCENT,
                            );
                        }
                        let color = if selected {
                            ui.visuals().text_color()
                        } else {
                            Color32::from_gray(190)
                        };
                        crate::paint_ellipsis(
                            ui,
                            Rect::from_min_max(rect.min + vec2(9.0, 0.0), close_rect.left_bottom()),
                            &tab.title,
                            13.0,
                            color,
                        );
                        let center = close_rect.center();
                        if close.hovered() {
                            painter.rect_filled(
                                Rect::from_center_size(center, vec2(20.0, 22.0)),
                                4.0,
                                Color32::from_gray(75),
                            );
                        }
                        let stroke = Stroke::new(1.3, color);
                        painter.line_segment(
                            [center - vec2(3.5, 3.5), center + vec2(3.5, 3.5)],
                            stroke,
                        );
                        painter.line_segment(
                            [center + vec2(-3.5, 3.5), center + vec2(3.5, -3.5)],
                            stroke,
                        );
                        body.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::Role::Button,
                                true,
                                selected,
                                &tab.title,
                            )
                        });
                        close.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                format!("Close {}", tab.title),
                            )
                        });
                        if close.clicked() || close.middle_clicked() || body.middle_clicked() {
                            action = Some(Action::Close(tab.id.clone()));
                        } else if body.clicked() {
                            action = Some(Action::Select(tab.id.clone()));
                        }
                        let mut tooltip = match server {
                            Some(server) => format!("{} · {server}", tab.title),
                            None => tab.title.clone(),
                        };
                        if index < NUMBER_KEYS.len() {
                            tooltip.push_str(&format!(" (Alt+{})", index + 1));
                        }
                        body.on_hover_text(tooltip);
                        close.on_hover_text("Close tab");
                    }
                });
                let mut reorder = None;
                if let Some((from, dragging)) = drag
                    && let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
                    && (viewport.top() - 10.0..=viewport.bottom() + 10.0).contains(&pointer.y)
                {
                    let gap = rects.partition_point(|rect| rect.center().x < pointer.x);
                    if dragging {
                        let x = rects
                            .get(gap)
                            .map_or_else(|| rects[gap - 1].right(), Rect::left);
                        ui.painter()
                            .vline(x, rects[from].y_range(), Stroke::new(2.0, ACCENT));
                        let edge = 20.0;
                        let edge_delta = (viewport.left() + edge - pointer.x).clamp(0.0, edge)
                            - (pointer.x - viewport.right() + edge).clamp(0.0, edge);
                        if edge_delta != 0.0 {
                            ui.scroll_with_delta(vec2(edge_delta / 3.0, 0.0));
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(16));
                        }
                    } else if viewport.expand(10.0).contains(pointer) {
                        reorder = Some((from, gap));
                    }
                }
                (action, reorder)
            });
        let (action, reorder) = scroll.inner;
        if let Some((from, gap)) = reorder {
            self.move_to_gap(from, gap);
            ui.ctx().request_repaint();
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_number_uses_tab_order_and_ignores_repeats_and_extra_modifiers() {
        let mut tabs = Tabs::default();
        let first = TabId::new("server-a", "first");
        let second = TabId::new("server-b", "second");
        tabs.open(first.clone());
        tabs.open(second.clone());
        let keypress = |key, modifiers, repeat| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        };
        let mut input = egui::InputState::default();
        input.events = vec![
            keypress(
                egui::Key::Num1,
                egui::Modifiers::ALT | egui::Modifiers::CTRL,
                false,
            ),
            keypress(egui::Key::Num9, egui::Modifiers::ALT, false),
            egui::Event::Text("9".into()),
            keypress(egui::Key::Num2, egui::Modifiers::ALT, true),
            egui::Event::Text("2".into()),
            egui::Event::Text("ordinary typing".into()),
        ];
        assert_eq!(tabs.consume_number_shortcut(&mut input), None);
        assert_eq!(input.events.len(), 2);
        assert_eq!(input.events[1], egui::Event::Text("ordinary typing".into()));

        for expected in [&second, &first] {
            input
                .events
                .push(keypress(egui::Key::Num2, egui::Modifiers::ALT, false));
            input.events.push(egui::Event::Text("2".into()));
            assert_eq!(
                tabs.consume_number_shortcut(&mut input).as_ref(),
                Some(expected)
            );
            assert_eq!(input.events.len(), 2);
            tabs.move_to_gap(0, 2);
        }
    }

    #[test]
    fn saved_layout_restores_order_and_active_chat_across_servers() {
        let work = TabId::new("work", "chat");
        let doom = TabId::new("doom", "chat");
        let closed = TabId::new("work", "closed");
        let mut tabs = Tabs::default();
        for id in [&work, &doom, &closed] {
            tabs.open(id.clone());
        }
        tabs.move_to_gap(1, 0);
        tabs.remove(&closed);
        let saved = serde_json::to_value(tabs.layout(&work)).unwrap();
        let (restored, active) =
            Tabs::restore(saved, |url| matches!(url, "work" | "doom")).unwrap();
        assert_eq!(restored.ids().collect::<Vec<_>>(), [&doom, &work]);
        assert_eq!(active, Some(work));
    }

    #[test]
    fn empty_layout_and_removed_server_do_not_restore_closed_tabs() {
        let blank = TabId::new("work", "draft:blank");
        let saved = serde_json::to_value(Tabs::default().layout(&blank)).unwrap();
        let (restored, active) = Tabs::restore(saved, |url| url == "work").unwrap();
        assert!(restored.is_empty());
        assert_eq!(active, Some(blank));

        let removed = TabId::new("old", "chat");
        let mut tabs = Tabs::default();
        tabs.open(removed.clone());
        tabs.open(TabId::new("work", "remaining"));
        let saved = serde_json::to_value(tabs.layout(&removed)).unwrap();
        let (restored, active) = Tabs::restore(saved, |url| url == "work").unwrap();
        assert_eq!(restored.ids().count(), 1);
        assert_eq!(active, Some(TabId::new("work", "remaining")));
    }

    #[test]
    fn pointer_drag_reorders_tabs_and_each_close_control_targets_its_tab() {
        let ctx = egui::Context::default();
        let first = TabId::new("work", "first");
        let second = TabId::new("work", "second");
        let third = TabId::new("doom", "third");
        let mut tabs = Tabs::default();
        for tab in [&first, &second, &third] {
            tabs.open(tab.clone());
        }
        let mut frame = |events| {
            let mut action = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 80.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = tabs.ui(ui, &first, |id| {
                        let title = match id.session.as_str() {
                            "first" => "One",
                            "second" => "Two",
                            _ => "Three",
                        };
                        (Some(title), None)
                    });
                },
            );
            output.textures_delta.clear();
            action
        };
        let move_to = |x| egui::Event::PointerMoved(pos2(x, 16.0));
        let click = |x, button, pressed| egui::Event::PointerButton {
            pos: pos2(x, 16.0),
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        frame(vec![]);
        frame(vec![move_to(30.0)]);
        frame(vec![click(30.0, PointerButton::Primary, true)]);
        frame(vec![move_to(230.0)]);
        assert!(frame(vec![click(230.0, PointerButton::Primary, false)]).is_none());

        // After dragging One to the end, the first tab body belongs to Two.
        frame(vec![move_to(30.0)]);
        frame(vec![click(30.0, PointerButton::Middle, true)]);
        assert!(matches!(
            frame(vec![click(30.0, PointerButton::Middle, false)]),
            Some(Action::Close(id)) if id == second
        ));

        // The middle tab's X belongs to Three.
        frame(vec![move_to(158.0)]);
        frame(vec![click(158.0, PointerButton::Primary, true)]);
        assert!(matches!(
            frame(vec![click(158.0, PointerButton::Primary, false)]),
            Some(Action::Close(id)) if id == third
        ));
        assert_eq!(tabs.ids().collect::<Vec<_>>(), [&second, &third, &first]);
    }
}
