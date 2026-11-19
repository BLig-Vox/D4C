<p align="center"><img src="assets/icons/256x256/site.blig.D4C.png" width="112" alt="D4C logo"></p>

# D4C

A native Rust client for Codex App Server. Chat, stream replies and tool activity, schedule work, and control isolated desktops from Linux or Android.

**Alpha software.** Linux and Android arm64 are the current targets. Your server runs Codex and keeps their state while the app is closed. iOS and Windows packages are not provided.

## Get started

1. Install the D4C manager and Codex CLI on your host, then sign in to Codex under the service account. See [server setup](docs/setup.md).
2. Download the Android arm64 APK or Linux x86_64 archive from [Releases](https://github.com/BLig-Vox/D4C/releases).
3. In **Settings → Add server**, enter the server name and private WSS URL. Existing installations keep their saved server addresses.
4. On a new device, compare its displayed fingerprint and approve the matching ID on the server using `d4c-server --approve DEVICE_ID`.

Select a chat under its server in the sidebar. Returning to a visited server reuses a healthy chat connection and fetches recent history. **Settings → Workspace server** sets an optional endpoint for desks and drafts. The default uses the same hostname on port 8443.

## Daily use

- **Chats:** start, search, rename or delete conversations in the sidebar, grouped under their server. Enter sends; Shift+Enter adds a line. The down-arrow returns to the latest message.
- **Models:** choose from the Codex server's model catalog in the composer. Your choice and reasoning level are remembered per connection. Availability comes from the server and your provider account.
- **Uploads:** use **+** or desktop drag-and-drop. Limits are 6 MB per file and 12 MB per draft. Sent images have previews; files have cards. Interpretation of audio, video and documents depends on the chosen runtime/model. Unsent attachments stay on their originating device.
- **Activity:** replies stream as Codex emits events. Tool calls have expandable details; thinking is displayed only when emitted by the server. Older messages load when scrolling up.
- **Commands:** type `/` to discover eligible server commands and skills. Select a suggestion, fill its arguments, then send. `/vnc connect NAME` attaches a desktop to the current conversation; `/vnc disconnect` detaches it without stopping the desktop.
- **Tasks:** ask in chat, such as “Every weekday at 9 AM, summarize my project.” Open the chat workspace's **Tasks** tab for status, history, run now, pause/resume and deletion. D4C schedules run in isolated sessions; results appear in Tasks history and notifications linked to the originating chat. Scheduled cards do not interrupt the conversation. Closing D4C does not stop a schedule.
- **Desktops:** open the chat workspace's **Desktop** tab, create or select an environment, and connect it. The floating monitor bubble is freely draggable. Tap to expand, minimize to return to the bubble, and resize/zoom the viewer. **Take over** pauses agent control; **Return to agent** explicitly hands it back. Minimized viewers do not stream video.
- **Notifications:** the bell shows recent activity. Settings controls OS alerts and sound. Android uses a foreground messaging service; Android notification permissions, battery rules, force-stop and network connectivity affect delivery.

## Codex and MCP

Use **Settings → Reload current session** on an idle conversation after changing its MCP configuration or skills. D4C asks Codex to refresh the tools for that conversation and keeps its thread ID. See [MCP setup](docs/mcp.md), [server deployment](docs/setup.md), and [Android notes](android/README.md).

## Prebuilt Linux install

Download `d4c-0.1.0-alpha.8-linux-x86_64.tar.gz` and `SHA256SUMS` from
[Releases](https://github.com/Blig-Vox/D4C/releases/tag/v0.1.0-alpha.8), then:

```sh
sha256sum --ignore-missing -c SHA256SUMS
tar -xzf d4c-0.1.0-alpha.8-linux-x86_64.tar.gz
cd d4c-0.1.0-alpha.8-linux-x86_64
python3 scripts/install-desktop.py bin/vox-d4c
~/.local/bin/vox-d4c
```

Requires glibc 2.39+ and compatible X11/Wayland and Vulkan drivers. This is a
native Linux build, not a static binary or AppImage. The separate workspace-server
archive includes its binary, deployment templates, scheduling tools, migration helpers and
setup documentation; configure it for your server using the guide below.

## Build and install on Linux

Requires a current stable Rust toolchain supporting edition 2024, Python 3, systemd with delegated cgroup v2 memory controls, a Vulkan-capable graphics stack and the usual C build tools. On Ubuntu, install the build dependencies and optional desktop integrations:

```sh
sudo apt install build-essential pkg-config libssl-dev libvulkan1 mesa-vulkan-drivers \
  libwayland-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev \
  zenity libnotify-bin pulseaudio-utils sound-theme-freedesktop
```

```sh
git clone https://github.com/Blig-Vox/D4C.git
cd D4C
python3 scripts/build-limited.py --limit-mb 2000 -- cargo build --locked -j 1
python3 scripts/install-desktop.py
```

The installer adds `~/.local/bin/d4c`, the logo, and a launcher in your app list. Run `~/.local/bin/d4c`. D4C opens fullscreen; the window manager can change that.

The build guard enforces **2,000,000,000 bytes RAM, zero build swap, one Cargo job**, and refuses to start below 4 GiB host `MemAvailable`. It requires Linux/systemd/cgroup v2 and fails closed if enforcement is unavailable. This is a build limit, not an app memory claim. You may request an optimized binary with `cargo build --release --locked -j 1` through the same guard, then install it with `python3 scripts/install-desktop.py target/release/vox-d4c`.

## Android

The release APK targets **arm64-v8a, Android 6+**, with physical interaction checks performed on Android 12. Install the APK on the phone and connect it to the same private network as your server. Enable notifications when prompted. It uses a native Android composer for keyboard suggestions, selection and emoji; glyph availability also depends on the installed Android font version.

```sh
rustup target add aarch64-linux-android
python3 scripts/build-android.py
adb install -r target/android/d4c-arm64-preview.apk
```

Install SDK platform 35/build-tools 35.0.0, NDK 28.2.13676358 and a JDK first. `ANDROID_HOME` and `ANDROID_NDK_HOME` override SDK/NDK locations. The same RAM guard applies. This alpha APK uses a developer signing key; retain the same key for in-place updates. No signing key, device identity, pairing state or server credential is shipped in the source.

## Architecture and limits

| Component | Responsibility |
| --- | --- |
| Rust egui/eframe client | Native UI, local device identity, rendering and input |
| Codex App Server | Native threads, model selection, turns, approvals, skills and MCP |
| D4C manager | Device pairing, encrypted transport, desktop bindings, drafts and notifications |
| D4C scheduler | Per-host job state, execution receipts and scheduled Codex work |
| Rootless Podman desktops | Independent Arch/XFCE homes, X11 sessions and working directories |

The client speaks the Codex App Server protocol over a device-authenticated WSS connection. The manager forwards it to a private Unix socket. Device identities stay in local app storage. Phone notification delivery depends on connectivity and Android background-service permissions.

Tested protocol version: Codex CLI 0.169.0. The legacy runtime is needed only to export data from an existing deployment. Preserve your migration snapshot until you have checked your conversations and scheduled jobs.

## License

MIT for D4C's original code. Vendored Rust patches and bundled fonts retain their own licenses; see [ATTRIBUTION.md](ATTRIBUTION.md), `vendor/` and the font license files in `assets/`.

### Chat permissions and desktop hardware

Open **Permissions** in the chat composer, or enter `/permissions` (alias `/permission`). Choose **Default** for workspace writes with approval requests, **Read only** for read-only sandboxing, or **Full access (YOLO)** for unsandboxed commands with network access and no approval prompts. Use `/permissions yolo on` and `/permissions yolo off` to toggle full access. Turning it off restores Default. Each selection applies to its Codex thread and takes effect for subsequent turns.

D4C desktops share the CPUs available to their host. On Linux, the workspace manager passes accessible DRM render devices into new desktop containers. Hardware acceleration requires a host GPU, compatible userspace drivers and support in the application. The VNC connection itself does not provide a GPU.
