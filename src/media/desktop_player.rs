//! GStreamer owns the synchronized audio/video clock. Both views paint the
//! same appsink texture; resizing the viewer never replaces the pipeline.
use eframe::egui;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::AppSink;
use std::path::Path;

pub struct Player {
    pipeline: gst::Element,
    sink: AppSink,
    texture: Option<egui::TextureHandle>,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub error: Option<String>,
    ended: bool,
}
impl Player {
    pub fn new(path: &Path) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let uri = gst::glib::filename_to_uri(path, None).map_err(|e| e.to_string())?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .field("width", gst::IntRange::new(1, 1920))
            .field("height", gst::IntRange::new(1, 1080))
            .build();
        let sink = AppSink::builder()
            .caps(&caps)
            .max_buffers(1)
            .drop(true)
            .build();
        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .property("video-sink", &sink)
            .build()
            .map_err(|e| e.to_string())?;
        let mut player = Self {
            pipeline,
            sink,
            texture: None,
            playing: false,
            position: 0.,
            duration: 0.,
            error: None,
            ended: false,
        };
        player.set_playing(true);
        Ok(player)
    }
    pub fn set_playing(&mut self, play: bool) {
        if play && self.ended {
            self.seek(0.);
        }
        let state = if play {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        if self.pipeline.set_state(state).is_err() {
            self.error = Some("Could not start the GStreamer video decoder".into());
            self.playing = false;
        } else {
            self.playing = play;
        }
    }
    pub fn seek(&mut self, seconds: f64) {
        let seconds = seconds.clamp(0., self.duration);
        if self
            .pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_mseconds((seconds * 1000.) as u64),
            )
            .is_ok()
        {
            self.ended = false;
            self.position = seconds;
        }
    }
    pub fn draw(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if let Some(bus) = self.pipeline.bus() {
            while let Some(message) = bus.pop() {
                match message.view() {
                    gst::MessageView::Error(e) => {
                        self.error = Some(e.error().to_string());
                        self.playing = false;
                        let _ = self.pipeline.set_state(gst::State::Null);
                    }
                    gst::MessageView::Eos(_) => {
                        self.playing = false;
                        self.ended = true;
                    }
                    _ => {}
                }
            }
        }
        if let Some(t) = self.pipeline.query_position::<gst::ClockTime>() {
            self.position = t.mseconds() as f64 / 1000.;
        }
        if let Some(t) = self.pipeline.query_duration::<gst::ClockTime>() {
            self.duration = t.mseconds() as f64 / 1000.;
        }
        let sample = self
            .sink
            .try_pull_sample(gst::ClockTime::ZERO)
            .or_else(|| self.sink.try_pull_preroll(gst::ClockTime::ZERO));
        if let Some(sample) = sample
            && let Some(image) = sample_image(&sample)
        {
            if let Some(texture) = self.texture.as_mut() {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture = Some(ui.ctx().load_texture(
                    "playing-video",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            super::viewer::paint_texture(ui, texture, rect, rect.center(), size * scale);
        }
    }
}
fn sample_image(sample: &gst::Sample) -> Option<egui::ColorImage> {
    let caps = sample.caps()?.structure(0)?;
    let width = caps.get::<i32>("width").ok()? as usize;
    let height = caps.get::<i32>("height").ok()? as usize;
    let pixels = sample.buffer()?.map_readable().ok()?;
    // RGBA output from videoconvert is four-byte aligned, with no row padding.
    let len = width.checked_mul(height)?.checked_mul(4)?;
    if pixels.len() != len {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &pixels,
    ))
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
