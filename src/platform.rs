use std::{io, path::PathBuf};

#[cfg(target_os = "android")]
static ANDROID_STORAGE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
thread_local! {
    static ANDROID_APP: std::cell::RefCell<Option<winit::platform::android::activity::AndroidApp>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "android")]
pub fn set_android_app(app: Option<winit::platform::android::activity::AndroidApp>) {
    ANDROID_APP.with(|slot| *slot.borrow_mut() = app);
}

#[cfg(target_os = "android")]
pub fn android_app() -> Option<winit::platform::android::activity::AndroidApp> {
    ANDROID_APP.with(|slot| slot.borrow().clone())
}

#[cfg(target_os = "android")]
pub fn attachment_cache() -> io::Result<PathBuf> {
    ANDROID_STORAGE
        .get()
        .and_then(|path| path.parent())
        .map(|root| root.join("cache/d4c-attachments"))
        .ok_or_else(|| io::Error::other("Android cache unavailable"))
}

pub fn install_symbol_fonts(fonts: &mut eframe::egui::FontDefinitions) {
    for (name, bytes) in [
        (
            "noto-symbols",
            include_bytes!("../assets/fonts/NotoSansSymbols2-Regular.ttf").as_slice(),
        ),
        (
            "nerd-symbols",
            include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf").as_slice(),
        ),
    ] {
        fonts.font_data.insert(
            name.into(),
            eframe::egui::FontData::from_static(bytes).into(),
        );
        for family in [
            eframe::egui::FontFamily::Proportional,
            eframe::egui::FontFamily::Monospace,
        ] {
            fonts.families.entry(family).or_default().push(name.into());
        }
    }
}

/// Android NativeActivity reports keyboard/system-bar bounds independently of
/// the rendering surface size. Winit 0.30 doesn't forward ContentRectChanged.
pub fn content_rect(ctx: &eframe::egui::Context) -> eframe::egui::Rect {
    let viewport = ctx.content_rect();
    #[cfg(target_os = "android")]
    return ANDROID_APP.with(|slot| {
        let app = slot.borrow();
        let Some(app) = app.as_ref() else {
            return viewport;
        };
        let bounds = app.content_rect();
        let scale = ctx.pixels_per_point();
        let bottom = crate::android_text::visible_bottom();
        let native_bottom = if bottom > 0 {
            bottom.min(bounds.bottom)
        } else {
            bounds.bottom
        };
        let visible = eframe::egui::Rect::from_min_max(
            eframe::egui::pos2(bounds.left as f32 / scale, bounds.top as f32 / scale),
            eframe::egui::pos2(bounds.right as f32 / scale, native_bottom as f32 / scale),
        )
        .intersect(viewport);
        if visible.is_positive() {
            visible
        } else {
            viewport
        }
    });
    #[cfg(not(target_os = "android"))]
    viewport
}

#[cfg(target_os = "android")]
pub fn set_android_storage(path: PathBuf) {
    let _ = ANDROID_STORAGE.set(path);
}

pub fn config_directory() -> io::Result<PathBuf> {
    #[cfg(target_os = "android")]
    {
        return ANDROID_STORAGE
            .get()
            .map(|path| path.join("d4c"))
            .ok_or_else(|| io::Error::other("Android private storage unavailable"));
    }
    #[cfg(not(target_os = "android"))]
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|path| path.join("d4c"))
        .ok_or_else(|| io::Error::other("Configuration directory unavailable"))
}
