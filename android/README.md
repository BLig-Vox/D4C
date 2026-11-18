# Android

Native Rust/egui arm64 client for the same OpenClaw and D4C workspace services used
by the desktop client. OpenGL ES rendering uses eframe NativeActivity. Device
identity and settings are app-private; backups and cleartext network traffic are
disabled. No server address or paired identity is bundled.

## Install the alpha

Download the APK from [Releases](https://github.com/Blig-Vox/D4C/releases), install
it, then follow the [server and pairing guide](../docs/setup.md). Join the same
Tailscale network as your server. Settings supports named WSS Gateway connections
and a separate workspace-server URL override for each Gateway.

The APK targets arm64-v8a and Android 6+. Physical interaction checks during
development used Android 12; other Android/keyboard/vendor combinations can vary.
This is a developer-signed alpha, not a Play Store build. Updates require the same
signing certificate. Do not uninstall to update if you want to preserve local data.

## Build on Linux

Install a JDK, Rust target, Android SDK platform 35 / build-tools 35.0.0, and NDK
28.2.13676358. SDK defaults to `$HOME/Android/Sdk`; `ANDROID_HOME` and
`ANDROID_NDK_HOME` override the paths.

```sh
rustup target add aarch64-linux-android
python3 scripts/build-android.py
adb install -r target/android/d4c-arm64-preview.apk
```

Run from the repository root. The bundled `scripts/build-limited.py` enforces
2,000,000,000 bytes RAM, zero build swap, one Cargo job and 4 GiB available host
memory before starting. It requires Linux/systemd/cgroup v2; enforcement failure
stops the build. `D4C_BUILD_GUARD` can select another compatible supervisor.

The packager compiles native code without a Gradle daemon, strips debug symbols,
packages uncompressed native libraries, aligns to 16 KB, signs with the developer's
local `~/.android/debug.keystore`, and verifies signing/alignment. The keystore is
private and excluded from Git. Preserve it for future in-place updates. Release
APKs disable Android debugging even though the signing certificate is a local
developer certificate.

Wireless ADB is optional. Use Android's Wireless debugging pairing popup's port
for `adb pair PHONE_IP:PAIRING_PORT`, enter the code locally, then use the main
Wireless debugging page's separate connection port for
`adb connect PHONE_IP:CONNECTION_PORT`. Do not commit pairing codes or addresses.

## Mobile behavior

- Native Android composer supports keyboard suggestions, selection, composition,
  emoji input and system clipboard copying. Other egui text fields are not native
  EditText controls. Keyboard settings still govern autocorrection/suggestions.
- The visible-window bounds keep the composer above the keyboard.
- A system document picker uploads files; sent image/file cards remain in history.
- Bundled symbol/emoji fallback and Android shaping handle common Unicode
  clusters, flags and skin tones. Newer unsupported emoji depend on OS coverage.
- Chat follows paginated server history; older prompts remain reachable during
  long tool runs. Activity groups expand without replacing user messages.
- The minimized monitor bubble is freely draggable inside the app. Tap to open,
  minimize with the viewer's minus button, and long press to dismiss. No Android
  overlay permission is needed. Minimized viewers release video resources.
- VNC supports fit/zoom, fullscreen, touch panning and explicit takeover. Input is
  authorized by the manager; Return to agent waits for server acknowledgement.
- Desktop/Tasks belong to the selected chat. Server bindings restore on reconnect.
- Scheduled work runs separately from live chat. Results appear in owner-linked
  notifications and Tasks history without adding cards to the conversation.
- A visible foreground messaging service receives background notifications, subject
  to Android permission, channel settings, network and power policy. Force-stop
  prevents delivery. Server work continues independently.

Vendored Android Activity lifecycle fixes are documented in
[ATTRIBUTION.md](../ATTRIBUTION.md) and `vendor/*/D4C-PATCH.md`. iOS packaging is not
implemented.
