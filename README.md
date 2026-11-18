<p align="center"><img src="assets/icons/256x256/site.blig.D4C.png" width="112" alt="D4C logo"></p>

# D4C

A native Rust client for your OpenClaw servers. Chat, stream replies and tool activity, schedule work, and control isolated desktops from Linux or Android.

**Alpha software.** Linux and Android arm64 are the current targets. A server runs your agents and keeps their state while the app is closed. iOS and Windows packages are not provided.

## Get started

1. [Set up OpenClaw](docs/setup.md#1-openclaw-and-codex-authentication) on your server.
2. Download the Android APK or Linux x86_64 archive from [Releases](https://github.com/Blig-Vox/D4C/releases).
3. Open **Settings → Add server**, enter a name and your `wss://` Gateway URL, save, and switch to it. Fresh installations contain no server address.
4. Approve the new device on the server with `openclaw devices list` and `openclaw devices approve REQUEST_ID`. Supply a Gateway token in Settings if your deployment requires one; tokens are not saved by D4C.
5. For shared drafts, desktop control and synchronized notification state, install the [workspace manager](docs/setup.md#3-persistent-workspace-and-desktops). It pairs devices separately.
6. Install the [MCP queue](docs/mcp.md) once on each server to connect MCP services from chat.

Use **Settings → Workspace server** if the manager runs on another hostname or port. Without an override, D4C uses the Gateway hostname on port `8443`. Saved connections and model/reasoning choices are device-local; conversations and agent work belong to OpenClaw.

## Daily use

- **Chats:** start, search, rename or delete conversations in the sidebar, grouped under their server. Enter sends; Shift+Enter adds a line. The down-arrow returns to the latest message.
- **Models:** choose from the connected Gateway's catalog in the composer. Your choice and reasoning level are remembered per connection. Availability comes from the server and your provider account.
- **Uploads:** use **+** or desktop drag-and-drop. Limits are 6 MB per file and 12 MB per draft. Sent images have previews; files have cards. Interpretation of audio, video and documents depends on the chosen runtime/model. Unsent attachments stay on their originating device.
- **Activity:** replies stream as Gateway events arrive. Tool calls have expandable details; thinking is displayed only when emitted by the server. Older messages load when scrolling up.
- **Commands:** type `/` to discover eligible server commands and skills. Select a suggestion, fill its arguments, then send. `/vnc connect NAME` attaches a desktop to the current conversation; `/vnc disconnect` detaches it without stopping the desktop.
- **Tasks:** ask in chat, such as “Every weekday at 9 AM, summarize my project.” Open the chat workspace's **Tasks** tab for status, history, run now, pause/resume and deletion. D4C schedules run in isolated sessions; results appear in Tasks history and notifications linked to the originating chat. Scheduled cards do not interrupt the conversation. Closing D4C does not stop a schedule.
- **Desktops:** open the chat workspace's **Desktop** tab, create or select an environment, and connect it. The floating monitor bubble is freely draggable. Tap to expand, minimize to return to the bubble, and resize/zoom the viewer. **Take over** pauses agent control; **Return to agent** explicitly hands it back. Minimized viewers do not stream video.
- **Notifications:** the bell shows recent activity. Settings controls OS alerts and sound. Android uses a foreground messaging service; Android notification permissions, battery rules, force-stop and network connectivity affect delivery.

## OpenClaw and MCP setup

- [Complete server setup and pairing](docs/setup.md)
- [MCP: install, connect, authenticate, check status and troubleshoot](docs/mcp.md)
- [Workspace manager details](server/README.md)
- [Android build and platform notes](android/README.md)

D4C uses OpenClaw's native MCP registry. The included helper queues connection changes until chat turns are idle, checks authentication and tool discovery, and then publishes the targeted change. A queued receipt does **not** mean the service is connected. Subsequent messages can inspect its applied/failed status. OAuth uses OpenClaw's native sign-in. Failed checks preserve the existing registry; the queue does not guarantee that every third-party server or upstream runtime is compatible.

## Prebuilt Linux install

Download `d4c-0.1.0-alpha.3-linux-x86_64.tar.gz` and `SHA256SUMS` from
[Releases](https://github.com/Blig-Vox/D4C/releases/tag/v0.1.0-alpha.3), then:

```sh
sha256sum --ignore-missing -c SHA256SUMS
tar -xzf d4c-0.1.0-alpha.3-linux-x86_64.tar.gz
cd d4c-0.1.0-alpha.3-linux-x86_64
python3 scripts/install-desktop.py bin/vox-d4c
~/.local/bin/vox-d4c
```

Requires glibc 2.39+ and compatible X11/Wayland and Vulkan drivers. This is a
native Linux build, not a static binary or AppImage. The separate workspace-server
archive includes its binary, deployment templates, plugin, migration helper and
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
| OpenClaw Gateway | Models, transcripts, active runs, tool permissions, MCP and schedules |
| D4C manager | SQLite workspace state, drafts, desktop bindings, notification/read synchronization |
| Rootless Podman desktops | Independent Arch/XFCE homes, workspaces, X11 and runtime directories |
| MCP queue worker | Durable status receipts, bounded discovery and idle configuration updates |

Each installation has its own device identity. Linux state is under the user's XDG config/data directories; Android state is app-private. Pairing is required on both services. WSS certificate checks remain enabled. The manager binds only to loopback or Tailscale addresses, and raw VNC stays server-local. Containers share the host kernel; this is not VM isolation. Immediate notifications need a connected client/service; durable server execution continues without it.

Compatibility was exercised with OpenClaw **2026.11.5** and Codex CLI **0.169.0**. Upstream interfaces can change. The queue currently supports credential-free HTTPS URLs with Streamable HTTP or SSE, public access or privately provisioned bearer tokens; use native OpenClaw sign-in for OAuth. Local stdio MCP is configured through OpenClaw, outside this helper.

## License

MIT for D4C's original code. Vendored Rust patches and bundled fonts retain their own licenses; see [ATTRIBUTION.md](ATTRIBUTION.md), `vendor/` and the font license files in `assets/`.
