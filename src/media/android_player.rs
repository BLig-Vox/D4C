//! A non-interactive native TextureView occupies only the visible video
//! rectangle. Controls and fullscreen layout remain in the common egui view.
use eframe::egui;
use jni::{
    JValue, jni_sig, jni_str,
    objects::{JObject, JString},
    refs::Global,
};
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicI64, Ordering},
    },
};

static NEXT: AtomicI64 = AtomicI64::new(1);
#[derive(Default)]
struct Status {
    id: i64,
    position: i32,
    duration: i32,
    playing: bool,
    error: Option<String>,
}
static STATUS: Mutex<Option<Status>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cVideo_report<'a>(
    mut env: jni::EnvUnowned<'a>,
    _class: jni::objects::JClass<'a>,
    id: i64,
    position: i32,
    duration: i32,
    playing: jni::sys::jboolean,
    error: JString<'a>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let error = error.try_to_string(env)?;
        if let Ok(mut status) = STATUS.lock()
            && let Some(slot) = status.as_mut().filter(|s| s.id == id)
        {
            *slot = Status {
                id,
                position,
                duration,
                playing,
                error: (!error.is_empty()).then_some(error),
            };
        }
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
enum Command {
    Open(String),
    Place([i32; 8]),
    Play(bool),
    Seek(i32),
    Close,
}
fn dispatch(id: i64, cmd: Command) -> Result<(), String> {
    let app = crate::platform::android_app().ok_or("Android activity unavailable")?;
    let target = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let vm = unsafe { jni::JavaVM::from_raw(target.vm_as_ptr().cast()) };
        let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let raw = target.activity_as_ptr() as jni::sys::jobject;
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
            let id = JValue::Long(id);
            match cmd {
                Command::Open(path) => {
                    let path = env.new_string(path)?;
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("openVideo"),
                        jni_sig!("(JLjava/lang/String;)V"),
                        &[id, JValue::Object(&path)],
                    )?;
                }
                Command::Place(bounds) => {
                    let mut args = vec![id];
                    args.extend(bounds.into_iter().map(JValue::Int));
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("placeVideo"),
                        jni_sig!("(JIIIIIIII)V"),
                        &args,
                    )?;
                }
                Command::Play(play) => {
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("playVideo"),
                        jni_sig!("(JZ)V"),
                        &[id, JValue::Bool(play)],
                    )?;
                }
                Command::Seek(ms) => {
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("seekVideo"),
                        jni_sig!("(JI)V"),
                        &[id, JValue::Int(ms)],
                    )?;
                }
                Command::Close => {
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("closeVideo"),
                        jni_sig!("(J)V"),
                        &[id],
                    )?;
                }
            }
            Ok(())
        });
        if result.is_err()
            && let Ok(mut status) = STATUS.lock()
            && let Some(s) = status.as_mut().filter(|s| s.id == id)
        {
            s.error = Some("Android video player unavailable".into());
            s.playing = false;
        }
    }));
    Ok(())
}
pub struct Player {
    id: i64,
    bounds: Option<[i32; 8]>,
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub error: Option<String>,
}
impl Player {
    pub fn new(path: &Path) -> Result<Self, String> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        *STATUS.lock().map_err(|_| "Android player unavailable")? = Some(Status {
            id,
            playing: true,
            ..Default::default()
        });
        dispatch(
            id,
            Command::Open(path.to_str().ok_or("Invalid attachment path")?.into()),
        )?;
        Ok(Self {
            id,
            bounds: None,
            position: 0.,
            duration: 0.,
            playing: true,
            error: None,
        })
    }
    pub fn draw(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if let Ok(status) = STATUS.lock()
            && let Some(s) = status.as_ref().filter(|s| s.id == self.id)
        {
            self.position = s.position.max(0) as f64 / 1000.;
            self.duration = s.duration.max(0) as f64 / 1000.;
            self.playing = s.playing;
            self.error = s.error.clone();
        }
        let clip = ui.clip_rect().intersect(rect);
        let scale = ui.ctx().pixels_per_point();
        let bounds = [
            rect.min.x, rect.min.y, rect.max.x, rect.max.y, clip.min.x, clip.min.y, clip.max.x,
            clip.max.y,
        ]
        .map(|v| (v * scale).round() as i32);
        if self.bounds != Some(bounds) {
            self.bounds = Some(bounds);
            let _ = dispatch(self.id, Command::Place(bounds));
        }
    }
    pub fn set_playing(&mut self, play: bool) {
        let _ = dispatch(self.id, Command::Play(play));
    }
    pub fn seek(&mut self, seconds: f64) {
        let _ = dispatch(
            self.id,
            Command::Seek((seconds.clamp(0., self.duration) * 1000.) as i32),
        );
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = dispatch(self.id, Command::Close);
    }
}
