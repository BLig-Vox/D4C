// Compile the same application as an Android NativeActivity shared library.
include!("main.rs");

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    // Android can start the replacement native thread before destroying the old
    // Activity. Keep renderer/editor globals owned by one Activity at a time.
    // This waits on the native thread, never on Android's UI thread, which must
    // remain free to deliver Destroy to the previous event loop.
    static ACTIVITY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _activity = ACTIVITY.lock().unwrap_or_else(|error| error.into_inner());
    let Some(storage) = app.internal_data_path() else {
        return;
    };
    android_text::detach();
    platform::set_android_storage(storage);
    platform::set_android_app(Some(app.clone()));
    let options = eframe::NativeOptions {
        android_app: Some(app),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    if let Err(error) = run(options) {
        eprintln!("D4C Android startup failed: {error}");
    }
    android_uploads::cancel();
    android_text::detach();
    eprintln!("D4C Android Activity stopped cleanly");
    platform::set_android_app(None);
}
