use super::{NamedTempFile, backend::Player};
use eframe::egui;
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Picture {
    name: String,
    texture: egui::TextureHandle,
    original: Option<Arc<Vec<u8>>>,
    loading: Option<mpsc::Receiver<Result<egui::ColorImage, String>>>,
    detail_error: Option<String>,
    zoom: f32,
    pan: egui::Vec2,
}
struct Movie {
    name: String,
    file: Arc<NamedTempFile>,
    player: Result<Player, String>,
    fullscreen: bool,
    visible: bool,
}
#[derive(Default)]
struct Viewer {
    picture: Option<Picture>,
    movie: Option<Movie>,
}
thread_local! { static VIEWER:RefCell<Viewer> = RefCell::new(Viewer::default()); }

pub fn reset() {
    VIEWER.with(|v| *v.borrow_mut() = Viewer::default());
}
pub fn is_fullscreen() -> bool {
    VIEWER.with(|v| {
        let v = v.borrow();
        v.picture.is_some() || v.movie.as_ref().is_some_and(|m| m.fullscreen)
    })
}
pub fn close_fullscreen() -> bool {
    VIEWER.with(|v| {
        let mut v = v.borrow_mut();
        if v.picture.take().is_some() {
            return true;
        }
        v.movie
            .as_mut()
            .is_some_and(|m| std::mem::take(&mut m.fullscreen))
    })
}
pub(super) fn open_image(
    name: String,
    texture: egui::TextureHandle,
    original: Option<Arc<Vec<u8>>>,
) {
    VIEWER.with(|v| {
        let mut v = v.borrow_mut();
        v.movie = None;
        v.picture = Some(Picture {
            name,
            texture,
            original,
            loading: None,
            detail_error: None,
            zoom: 1.,
            pan: egui::Vec2::ZERO,
        });
    });
}
pub(super) fn play(file: Arc<NamedTempFile>, name: String) {
    VIEWER.with(|v| {
        let mut v = v.borrow_mut();
        v.movie = None;
        let player = Player::new(file.path());
        v.movie = Some(Movie {
            name,
            file,
            player,
            fullscreen: false,
            visible: true,
        });
    });
}
pub(super) fn inline(ui: &mut egui::Ui, name: &str, file: &Arc<NamedTempFile>) {
    let active = VIEWER.with(|v| {
        let mut v = v.borrow_mut();
        let Some(movie) = v.movie.as_mut().filter(|m| Arc::ptr_eq(&m.file, file)) else {
            return false;
        };
        let width = ui.available_width().min(420.).max(1.);
        let size = egui::vec2(width, (width * 0.5625).max(48.));
        let visible = ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, size));
        if visible && !movie.fullscreen {
            movie.visible = true;
            video_ui(ui, movie, size);
        } else {
            ui.allocate_space(size);
        }
        true
    });
    if !active && ui.button("Play video").clicked() {
        play(file.clone(), name.into());
        ui.ctx().request_repaint();
    }
}
fn time(s: f64) -> String {
    let sec = s.max(0.) as u64;
    format!("{}:{:02}", sec / 60, sec % 60)
}
fn video_ui(ui: &mut egui::Ui, movie: &mut Movie, size: egui::Vec2) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(rect, 0., egui::Color32::BLACK);
    let player = match &mut movie.player {
        Ok(p) => p,
        Err(e) => {
            ui.label(e.as_str());
            return;
        }
    };
    player.draw(ui, rect);
    if let Some(error) = &player.error {
        ui.label(error);
        return;
    }
    if response.clicked() {
        player.set_playing(!player.playing);
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(if player.playing { "Pause" } else { "Play" })
            .clicked()
        {
            player.set_playing(!player.playing);
        }
        if ui.button("−10 s").clicked() {
            player.seek(player.position - 10.);
        }
        if ui.button("+10 s").clicked() {
            player.seek(player.position + 10.);
        }
        if !movie.fullscreen && ui.button("Full screen").clicked() {
            movie.fullscreen = true;
        }
    });
    ui.horizontal(|ui| {
        let label = format!("{} / {}", time(player.position), time(player.duration));
        ui.spacing_mut().slider_width = (ui.available_width() - 85.).max(32.);
        let mut position = player.position;
        if ui
            .add_enabled(
                player.duration > 0.,
                egui::Slider::new(&mut position, 0.0..=player.duration.max(0.01)).show_value(false),
            )
            .changed()
        {
            player.seek(position);
        }
        ui.small(label);
    });
    ui.ctx()
        .request_repaint_after(Duration::from_millis(if player.playing { 32 } else { 200 }));
}
pub(super) fn paint_texture(
    ui: &egui::Ui,
    texture: &egui::TextureHandle,
    clip: egui::Rect,
    center: egui::Pos2,
    size: egui::Vec2,
) {
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(clip))
        .image(
            texture.id(),
            egui::Rect::from_center_size(center, size),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            egui::Color32::WHITE,
        );
}
fn image_ui(ui: &mut egui::Ui, picture: &mut Picture) {
    static DECODING: AtomicBool = AtomicBool::new(false);
    if picture.original.is_some() {
        if DECODING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            let original = picture.original.take().unwrap();
            let side = ui.input(|i| i.max_texture_side.min(4096)) as u32;
            let ctx = ui.ctx().clone();
            let (tx, rx) = mpsc::channel();
            picture.loading = Some(rx);
            std::thread::spawn(move || {
                let result = super::decode_image(&original, side).map(|(image, _)| image);
                let _ = tx.send(result);
                DECODING.store(false, Ordering::Release);
                ctx.request_repaint();
            });
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(80));
        }
    }
    if let Some(result) = picture.loading.as_ref().and_then(|rx| rx.try_recv().ok()) {
        picture.loading = None;
        match result {
            Ok(image) => {
                picture.texture =
                    ui.ctx()
                        .load_texture("fullscreen-image", image, egui::TextureOptions::LINEAR)
            }
            Err(e) => picture.detail_error = Some(e),
        }
    }
    let mut factor = 1.;
    ui.horizontal(|ui| {
        if ui.button("−").on_hover_text("Zoom out").clicked() {
            factor = 0.8;
        }
        if ui.button("+").on_hover_text("Zoom in").clicked() {
            factor = 1.25;
        }
        if ui.button("Fit").clicked() {
            picture.zoom = 1.;
            picture.pan = egui::Vec2::ZERO;
        }
        ui.label(format!("{:.0}%", picture.zoom * 100.));
        if picture.loading.is_some() {
            ui.spinner();
        }
        if let Some(e) = &picture.detail_error {
            ui.weak(e);
        }
    });
    let (rect, resp) = ui.allocate_exact_size(
        ui.available_size().max(egui::Vec2::splat(1.)),
        egui::Sense::click_and_drag(),
    );
    let mut anchor = rect.center();
    if let Some(t) = ui
        .input(|i| i.multi_touch())
        .filter(|t| rect.contains(t.start_pos))
    {
        factor *= t.zoom_delta;
        anchor = t.center_pos;
        picture.pan += t.translation_delta;
    } else {
        picture.pan += resp.drag_delta();
        if resp.hovered() {
            factor *= ui.input(|i| i.zoom_delta() * (i.smooth_scroll_delta.y * 0.003).exp());
            anchor = resp.hover_pos().unwrap_or(anchor);
        }
        if resp.double_clicked() {
            factor = if picture.zoom > 1. {
                1. / picture.zoom
            } else {
                2.
            };
        }
    }
    let old = picture.zoom;
    picture.zoom = (old * factor).clamp(1., 16.);
    let offset = anchor - rect.center();
    picture.pan = offset + (picture.pan - offset) * (picture.zoom / old);
    let size = picture.texture.size_vec2();
    let fit = (rect.width() / size.x).min(rect.height() / size.y);
    let size = size * fit * picture.zoom;
    let limit = ((size - rect.size()) * 0.5).max(egui::Vec2::ZERO);
    picture.pan = picture.pan.clamp(-limit, limit);
    paint_texture(
        ui,
        &picture.texture,
        rect,
        rect.center() + picture.pan,
        size,
    );
}
pub fn finish_frame(ctx: &egui::Context) {
    VIEWER.with(|v| {
        let mut v = v.borrow_mut();
        let full = v.picture.is_some() || v.movie.as_ref().is_some_and(|m| m.fullscreen);
        if full {
            let bounds = crate::platform::content_rect(ctx);
            let id = egui::Id::new("attachment-viewer");
            let mut close = false;
            let modal = egui::Modal::new(id)
                .area(
                    egui::Area::new(id)
                        .order(egui::Order::Foreground)
                        .fixed_pos(bounds.min)
                        .movable(false),
                )
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_gray(15))
                        .inner_margin(8),
                )
                .show(ctx, |ui| {
                    let size = (bounds.size() - egui::Vec2::splat(16.)).max(egui::Vec2::splat(1.));
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    ui.horizontal(|ui| {
                        close = ui.button("Close").clicked();
                        let name = v
                            .picture
                            .as_ref()
                            .map(|p| p.name.as_str())
                            .or_else(|| v.movie.as_ref().map(|m| m.name.as_str()))
                            .unwrap_or("");
                        ui.add(egui::Label::new(name).truncate());
                    });
                    if let Some(p) = v.picture.as_mut() {
                        image_ui(ui, p);
                    } else if let Some(movie) = v.movie.as_mut() {
                        let size =
                            egui::vec2(ui.available_width(), (ui.available_height() - 76.).max(1.));
                        video_ui(ui, movie, size);
                        movie.visible = true;
                    }
                });
            if close || modal.should_close() {
                v.picture = None;
                if let Some(m) = v.movie.as_mut() {
                    m.fullscreen = false;
                }
            }
        }
        if let Some(m) = v.movie.as_mut() {
            if !m.visible {
                v.movie = None;
            } else {
                m.visible = false;
            }
        }
    });
}
