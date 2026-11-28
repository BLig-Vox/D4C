# D4C workspace manager

The manager authenticates devices and serves chat, workspace state and desktops on a single TLS listener. The Codex app server runs as the unprivileged `d4c` account and owns native conversations, model access and approvals. Each paired chat client connects to a private app-server Unix socket through the manager. Chat continues when a client disconnects.

| Endpoint | Purpose |
| --- | --- |
| `/` or `/codex` | Paired native Codex app-server messages |
| `/control` | Workspace state, drafts, tasks and notifications |
| `/vnc/<desktop-id>` | Paired viewer for a persistent desktop |
| `/run/d4c/manager.sock` | Local manager IPC |
| `/run/d4c-codex/app.sock` | UID-private Codex listener |

The deployment uses `/var/lib/d4c` for manager state and `/var/lib/d4c/codex` for Codex credentials, workspace and history. The service mount namespace hides manager keys, the raw manager socket and manager database from shell tools. An additional systemd socket on the tailnet interface can preserve a previously saved chat URL.

The timer and `/usr/local/libexec/d4c-scheduler` run once per server. Scheduled turns have separate run history. Each desktop uses its own rootless Podman container, home, workspace, X server and DBus session. There is no separate limit on how many saved desktops can run at once. Each desktop has its own PID limit. CPU, RAM and swap share the host’s available capacity. Accessible Linux DRM render nodes are passed through for compatible GPU workloads. The build supervisor's memory cap applies during compilation.

Human takeover is enforced server-side. When a controller lease expires, the manager releases held input and keeps agent input paused until a user selects **Return to agent**. Deleting a desktop removes the unit and container and retains its files.

Use the [deployment and pairing guide](../docs/setup.md) for prerequisites, rendered systemd units, migration, sign-in and validation. See the [MCP guide](../docs/mcp.md) for external tool configuration.

The optional `server/scripts/verify-live.mjs` checks desktop lifecycle, synchronization and takeover on a test host. It creates temporary identities and a fixture desktop and restarts the manager. Read the script before running it and set `D4C_HOSTNAME` to the certificate hostname.

## Native Linux desktop (X11)

To share an existing X11 session without Podman or TigerVNC, run the native screen bridge as the same user as the manager, with access to the session's `DISPLAY` and X authority:

```sh
install -d -m 700 "${XDG_RUNTIME_DIR}/d4c-native"
d4c-server --native-screen "${XDG_RUNTIME_DIR}/d4c-native/host-screen.sock"
```

The bridge accepts connections only through that user-private Unix socket. Configure the manager's `D4C_HOST_DESKTOP_ID` to the ID of an **existing** workspace desktop record and `D4C_HOST_SCREEN_SOCKET` to the absolute socket path. If the socket path is unset, the manager still uses `host-screen.sock` inside its data directory. Other desktop records continue to use the container backend.

For lifecycle control from the desktop panel, run the bridge under the user unit named `d4c-<desktop-id>.service`, with the display credentials and a private socket directory (for example, a systemd `RuntimeDirectory=` with mode `0700`). If you run it manually, remove a stale socket only after verifying the previous bridge has stopped. No host desktop is registered or shared automatically.

This backend shares the X11 root screen (up to 2560×1600) with mapped keyboard and pointer input. It does not capture a native Wayland desktop and does not provide audio or clipboard synchronization.
