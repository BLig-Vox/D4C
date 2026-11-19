# Changelog

## 0.1.0-alpha.14

- Align Markdown bullets with the first line of wrapped list items.
- Keep model and reasoning controls on one row on narrow screens.

## 0.1.0-alpha.13

- Model, reasoning effort and permissions now open directly above their desktop composer controls.
- Compact layouts retain bottom sheets, with full-width options and a checkmark beside the current selection.

## 0.1.0-alpha.12

- Search chats across servers with Ctrl+K on desktop, a visible focus ring and an inline clear button.
- Updated the chat composer with compact attachment, permission, model and reasoning controls and a right-aligned send button.
- Centered the sidebar chat menu alongside the title and selection indicator.

## 0.1.0-alpha.11

- Open chat links on Android, inspect images at full size, and play attached videos with seeking and fullscreen controls.
- Read Markdown questions and approvals in the conversation and select the exact response offered by Codex.
- Updated native dialogs and mobile sheets using shared shadcn-style controls.
- The workspace server can deliver shared media and desktop screenshots over the paired chat connection.

## 0.1.0-alpha.10

- Keep authenticated chat connections open across server switches and reuse workspace connections when returning to a server.
- Load the selected conversation first. New chats open directly in the composer.
- Android arm64 version code 15.

## 0.1.0-alpha.9

- Switch Codex permissions per chat, including full access with `/permissions yolo on` and default permissions with `/permissions yolo off`.
- Share host CPU capacity across desktops and pass accessible Linux GPU render devices into new desktop containers.
- Add space between the reply indicator and the composer. Android arm64 version code 14.

- Reuse a per-desktop VNC connection for fresh screenshots. Recover a lost capture connection once and back off failed VNC handshakes across viewer, screenshot, and input requests.

- Restore native desktop tools from migrated compaction checkpoints and relocate saved workspace settings when resuming a chat. Older desktop tool names use the same conversation binding and human takeover checks.

- Page full Codex history one turn at a time and replace embedded desktop-tool screenshots with a text marker during replay. This prevents screenshot-heavy conversations from repeatedly disconnecting when switching servers. Original screenshots remain in Codex history.

- Preserve DNS resolution inside the isolated Codex service on systemd-resolved hosts.
- Keep current chats visible after migration and archive other imported rollouts. Exclude Codex subagent threads from sidebar queries.

## 0.1.0-alpha.8

- Run chats on native Codex App Server over the saved WSS endpoint. D4C uses its paired device identity for chat and workspace access.
- Resume native threads and map migrated selections, desktop bindings, and drafts back to their conversations.
- Discover native models and skills, stream replies and tool activity, and answer approval requests in chat. Refresh MCP connections from an idle session.
- Include the standalone D4C scheduler, migration tools, and Linux service definitions in the server archive.
- Android arm64 version code 13. Use a matching direct-Codex server for this client.

## 0.1.0-alpha.7

- Reuse the authenticated chat connection when returning to a previously visited server and refresh its conversation history.
- Include an offline migration staging tool for existing OpenClaw chats, the bundled Ponytail skill, and an optional paired transport to a local Codex App Server. The existing chat runtime continues to serve this release.
- Android version code 12.

## 0.1.0-alpha.6 — 2026-11-19

- Display failed replies in the transcript, both live and after reopening a chat.
- Respect display flags on custom notices; deduplicate persisted and live errors
  by run identity and recover missed failures from the session snapshot.
- Add a dry-run-first helper for reconciling Codex-routed OpenAI model entries
  against a fresh account catalog, preserving other providers and making a backup.
- Recover Codex app-server request timeouts on the server after all tracked turns
  finish. Refresh only managed child connections, with cooldown and hourly limits;
  notify the originating chat without replaying prompts or tool actions.
- Android version code 11; existing signing identity is preserved.

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
