# Changelog

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
