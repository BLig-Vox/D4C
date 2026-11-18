# Third-party source

## winit 0.30.13
- Creator: rust-windowing / winit contributors.
- Source: https://github.com/rust-windowing/winit/tree/v0.30.13
- Obtained from the installed crates.io package on 2026-11-18.
- License: Apache-2.0, preserved in vendor/winit/LICENSE.
- Modifications: Android Destroy exits its native event loop and releases the
  one-loop creation guard for the replacement Activity. See vendor/winit/D4C-PATCH.md.

## eframe 0.37.0
- Creator: egui contributors.
- Source: https://github.com/emilk/egui/tree/0.37.0/crates/eframe
- Obtained from the installed crates.io package on 2026-11-18.
- License: Apache-2.0 OR MIT; upstream license files preserved in vendor/eframe.
- Modifications: tolerate a missing window during shutdown; restore an Android
  surfaceless EGL context for GPU resource cleanup; provide a wakeable Android
  Activity exit signal. See vendor/eframe/D4C-PATCH.md.

## Fonts and icons

Bundled fonts preserve their license files in assets/fonts and assets/android-fonts.
Lucide icons are supplied by the lucide-icons dependency; see its upstream ISC license.
The selected D4C app artwork is recorded in assets/branding/README.md.
