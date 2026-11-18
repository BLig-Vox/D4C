# Android renderer shutdown patch

Source: eframe 0.37.0, egui contributors.
https://github.com/emilk/egui/tree/0.37.0/crates/eframe
Copied from the installed crates.io source on 2026-11-18.
Upstream Apache-2.0/MIT licenses and copyright notices are preserved.

save_and_destroy now saves with an optional window, matching WinitApp::save.
Android may already have removed that window and its GL surface on Suspended.
For Android, make the retained EGL context current without a surface before
calling app.on_exit and destroying painter resources. If unavailable, skip GL
callbacks: EGL context destruction releases the driver objects instead.
Other platforms retain their current GL cleanup path.

Android also exposes request_android_activity_exit, a wakeable event-loop exit
used when the Java Activity is finishing. This avoids waiting for a rendered
frame or onDestroy: Android may create the replacement before destroying the
old Activity. Its proxy is released when the loop exits. D4C serializes native
Activity lifetimes so renderer/editor globals cannot overlap.
