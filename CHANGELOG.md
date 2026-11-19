# Changelog

## 0.1.0-alpha.5 — 2026-11-19

- Negotiate bounded lossless compression for desktop video transport, with raw
  compatibility for earlier clients and servers. Decode and validate each chunk.
- Keep first-frame downloads alive while data makes progress.
- Restore recent chat and desktop placement per server, including rapid switches
  and empty chats. Cache at most two inactive servers and 100 rows / 16 MiB each.
- Replace desktop loading spinners with responsive skeletons, compact recovery
  controls, and a retained preview when reopening a minimized viewer.
- Preserve takeover protections until a fresh desktop frame arrives.
- Android version code 10; existing app data and signing identity are preserved.

## 0.1.0-alpha.4 — 2026-11-19

- Recover temporary WebSocket write failures by flushing the queued frame once;
  never duplicate a message after a partial write. Shared by chat, workspace and server.
- Add server workspace/VNC heartbeats and log peer addresses.
- Allow slower authentication and idle recovery on lossy routes.
- Keep conversation history visible during reconnects; replace chat loading spinners
  and the reconnect configuration form with compact skeleton placeholders.
- Report the installed client version in the Gateway handshake. Android version code 9.


## 0.1.0-alpha.3 — 2026-11-19

- Bound Gateway TCP/TLS connection and authentication waits; automatically retry
  silent connections with heartbeat monitoring.
- Wait for authentication before sending queued RPCs after a reconnect.
- Remove the unconditional device-approval reminder from disconnected screens;
  show connection progress and actual Gateway errors instead.
- Remove D4C desktop RAM/swap caps, including persistent migration of existing
  containers without restarting their desktops. Compilation guards are unchanged.
- Android version code 8, signed with the same developer key as earlier APKs.


## 0.1.0-alpha.1 — 2026-11-19

First public D4C alpha for Linux and Android arm64.

- Native chat with Markdown, code copying, streamed activity, attachments,
  transcript pagination, session search, renaming and deletion.
- Multiple named OpenClaw servers, discovered models and remembered reasoning.
- Server-owned workspace state, drafts, desktop bindings and notifications.
- Session-attached Arch/XFCE desktops with server-enforced human takeover,
  resizable VNC windows and freely draggable in-app monitor bubbles.
- Android keyboard-aware native composer, emoji fallback, file picker,
  attachment previews and foreground-service alerts.
- Persistent MCP queue: idle application, private credential handoff,
  bounded discovery/recovery and explicit queued/applied/failed receipts.
- Fresh installs have no personal server endpoint. Workspace URLs, deployment
  host/UID, plugin socket and OpenClaw package location are configurable.
- Setup guides for OpenClaw/Codex, pairing, workspace manager and MCP.

Alpha limitations: Linux/systemd server deployment; arm64 Android APK with a
local developer signing key; no iOS/Windows packages. Android background delivery
is subject to OS/network policy. MCP OAuth uses native OpenClaw sign-in. Upstream
runtime compatibility is validated against OpenClaw 2026.11.5 / Codex 0.169.0;
third-party MCP services can still reject credentials or be unavailable.
