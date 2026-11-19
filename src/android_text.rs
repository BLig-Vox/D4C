//! Real Android EditText for the composer; NativeActivity's dummy IME has no
//! surrounding-text/composition support. Text stays scoped to its chat token.
use eframe::egui;
use jni::{
    JValue, jni_sig, jni_str,
    objects::{JObject, JString},
    refs::Global,
};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
#[derive(Clone, PartialEq)]
struct Editor {
    session: String,
    text: String,
    bounds: [i32; 4],
    revision: u64,
    focus: bool,
}
static DESIRED: Mutex<Option<Editor>> = Mutex::new(None);
static LAST: Mutex<Option<Editor>> = Mutex::new(None);
static EDIT: Mutex<Option<(String, String, u64)>> = Mutex::new(None);
static VISIBLE_BOTTOM: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
pub fn visible_bottom() -> i32 {
    VISIBLE_BOTTOM.load(Ordering::Acquire)
}
static APPLIED_REVISION: AtomicU64 = AtomicU64::new(0);
static CONTEXT: Mutex<Option<egui::Context>> = Mutex::new(None);
fn wake() {
    let ctx = CONTEXT.lock().ok().and_then(|slot| slot.clone());
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}
/// The Android process can outlive an Activity because notifications stay alive.
/// Do not retain its editor, font rasterizer or repaint proxy across recreation.
pub fn detach() {
    *CONTEXT.lock().unwrap() = None;
    *DESIRED.lock().unwrap() = None;
    *LAST.lock().unwrap() = None;
    *EDIT.lock().unwrap() = None;
    VISIBLE_BOTTOM.store(0, Ordering::Release);
    set_modal_visible(false);
    take_modal_back();
    APPLIED_REVISION.store(0, Ordering::Release);
}
pub fn begin(ctx: &egui::Context) -> Option<(String, String, u64)> {
    *CONTEXT.lock().unwrap() = Some(ctx.clone());
    *DESIRED.lock().unwrap() = None;
    EDIT.lock().ok()?.take()
}
// Only acknowledge a revision together with the text applied by the render thread.
// A newer Java callback must never label an older Rust draft as current.
pub fn acknowledge(revision: u64) {
    APPLIED_REVISION.store(revision, Ordering::Release);
}
pub fn show(session: &str, text: &str, rect: egui::Rect, scale: f32, focus: bool) {
    if !rect.is_positive() {
        return;
    }
    *DESIRED.lock().unwrap() = Some(Editor {
        session: session.into(),
        text: text.into(),
        bounds: [
            (rect.left() * scale).round() as i32,
            (rect.top() * scale).round() as i32,
            (rect.width() * scale).round().max(1.0) as i32,
            (rect.height() * scale).round().max(1.0) as i32,
        ],
        revision: APPLIED_REVISION.load(Ordering::Acquire),
        focus,
    });
}
pub fn finish() {
    let desired = DESIRED.lock().unwrap().take();
    {
        let mut last = LAST.lock().unwrap();
        if *last == desired {
            return;
        }
        *last = desired.clone();
    }
    let Some(app) = crate::platform::android_app() else {
        return;
    };
    let activity_app = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let vm = unsafe { jni::JavaVM::from_raw(activity_app.vm_as_ptr().cast()) };
        let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let raw = activity_app.activity_as_ptr() as jni::sys::jobject;
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
            if let Some(e) = desired {
                let token = env.new_string(&e.session)?;
                let text = env.new_string(&e.text)?;
                env.call_method(
                    activity.as_ref(),
                    jni_str!("renderComposer"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;IIIIJZ)V"),
                    &[
                        JValue::Object(&token),
                        JValue::Object(&text),
                        JValue::Int(e.bounds[0]),
                        JValue::Int(e.bounds[1]),
                        JValue::Int(e.bounds[2]),
                        JValue::Int(e.bounds[3]),
                        JValue::Long(e.revision as i64),
                        JValue::Bool(e.focus),
                    ],
                )?;
            } else {
                env.call_method(
                    activity.as_ref(),
                    jni_str!("hideComposer"),
                    jni_sig!("()V"),
                    &[],
                )?;
            }
            Ok(())
        });
        if let Err(e) = result {
            eprintln!("D4C native composer: {e}");
        }
    }));
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_composerChanged<'a>(
    mut env: jni::EnvUnowned<'a>,
    _this: JObject<'a>,
    token: JString<'a>,
    text: JString<'a>,
    revision: jni::sys::jlong,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let session = token.try_to_string(env)?;
        let text = text.try_to_string(env)?;
        if text.len() <= 256 * 1024 && revision >= 0 {
            if let Ok(mut edit) = EDIT.lock() {
                *edit = Some((session, text, revision as u64));
            }
            wake();
        }
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_visibleBottomChanged(
    _env: jni::EnvUnowned<'_>,
    _this: JObject<'_>,
    bottom: jni::sys::jint,
) {
    if VISIBLE_BOTTOM.swap(bottom, Ordering::AcqRel) != bottom {
        wake();
    }
}

pub fn install_emoji(ctx: &egui::Context) {
    let Some(app) = crate::platform::android_app() else {
        return;
    };
    for color in [true, false] {
        let app = app.clone();
        ctx.add_glyph_rasterizer(
            epaint::text::GlyphRasterizer::new(
                if color {
                    "site.blig.D4C.AndroidEmoji"
                } else {
                    "site.blig.D4C.AndroidFallback"
                },
                move |request| {
                    if matches!(request.family, egui::FontFamily::Name(_)) {
                        return None;
                    }
                    if color
                        && request.cluster != "🗐"
                        && !epaint::text::has_emoji_presentation(request.cluster)
                    {
                        return None;
                    }
                    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
                    let values = vm
                        .attach_current_thread(|env| -> jni::errors::Result<Vec<i32>> {
                            let raw = app.activity_as_ptr() as jni::sys::jobject;
                            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
                            let text = env.new_string(request.cluster)?;
                            let object = env
                                .call_method(
                                    activity.as_ref(),
                                    jni_str!("rasterizeEmoji"),
                                    jni_sig!("(Ljava/lang/String;FZ)[I"),
                                    &[
                                        JValue::Object(&text),
                                        JValue::Float(request.font_size_px),
                                        JValue::Bool(color),
                                    ],
                                )?
                                .l()?;
                            let array = jni::objects::JIntArray::cast_local(env, object)?;
                            let len = env.get_array_length(&array)?;
                            if len < 5 || len > 1048581 {
                                return Ok(Vec::new());
                            }
                            let mut values = vec![0; len as usize];
                            env.get_int_array_region(&array, 0, &mut values)?;
                            Ok(values)
                        })
                        .ok()?;
                    if values.len() < 5 {
                        return None;
                    }
                    let width = usize::try_from(values[0]).ok()?;
                    let height = usize::try_from(values[1]).ok()?;
                    if width * height != values.len() - 5 {
                        return None;
                    }
                    let rgba: Vec<u8> = values[5..]
                        .iter()
                        .flat_map(|v| {
                            let c = *v as u32;
                            [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
                        })
                        .collect();
                    Some(epaint::text::RasterizedGlyph {
                        bitmap: epaint::text::GlyphBitmap {
                            image: egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba),
                            offset_px: egui::vec2(values[3] as f32, values[4] as f32),
                            is_color: color,
                        },
                        advance_px: values[2] as f32 / 1000.0,
                    })
                },
            )
            .with_priority(if color {
                epaint::text::FontPriority::Highest
            } else {
                epaint::text::FontPriority::Lowest
            }),
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_composerWindowHidden(
    _env: jni::EnvUnowned<'_>,
    _this: JObject<'_>,
) {
    if let Ok(mut last) = LAST.lock() {
        *last = None;
    }
    wake();
}

/// egui-winit uses an in-process clipboard on Android. Forward all UI copy
/// commands (including Markdown code blocks) to the actual Android clipboard.
pub fn flush_platform_commands(ctx: &egui::Context) {
    let texts = ctx.output_mut(|output| {
        let mut texts = Vec::new();
        output.commands.retain(|command| {
            if let egui::OutputCommand::CopyText(text) = command {
                texts.push((false, text.clone()));
                false
            } else if let egui::OutputCommand::OpenUrl(link) = command {
                texts.push((true, link.url.clone()));
                false
            } else {
                true
            }
        });
        texts
    });
    let Some(app) = crate::platform::android_app() else {
        return;
    };
    for (is_link, text) in texts {
        let activity_app = app.clone();
        app.run_on_java_main_thread(Box::new(move || {
            let vm = unsafe { jni::JavaVM::from_raw(activity_app.vm_as_ptr().cast()) };
            let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let raw = activity_app.activity_as_ptr() as jni::sys::jobject;
                let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
                let text = env.new_string(&text)?;
                env.call_method(
                    activity.as_ref(),
                    if is_link {
                        jni_str!("openChatLink")
                    } else {
                        jni_str!("copyText")
                    },
                    jni_sig!("(Ljava/lang/String;)V"),
                    &[JValue::Object(&text)],
                )?;
                Ok(())
            });
            if let Err(error) = result {
                eprintln!("D4C Android platform action: {error}");
            }
        }));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_finishNativeUi(
    _env: jni::EnvUnowned<'_>,
    _this: JObject<'_>,
) {
    eframe::request_android_activity_exit();
}

static MODAL_VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MODAL_BACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn set_modal_visible(visible: bool) {
    MODAL_VISIBLE.store(visible, Ordering::Release);
}
pub fn take_modal_back() -> bool {
    MODAL_BACK.swap(false, Ordering::AcqRel)
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_dismissChatOverlay(
    _env: jni::EnvUnowned<'_>,
    _this: JObject<'_>,
) -> jni::sys::jboolean {
    let consume = MODAL_VISIBLE.load(Ordering::Acquire);
    if consume {
        MODAL_BACK.store(true, Ordering::Release);
        wake();
    }
    consume.into()
}
