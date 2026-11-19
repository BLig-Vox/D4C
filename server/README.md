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

The timer and `/usr/local/libexec/d4c-scheduler` run once per server. Scheduled turns have separate run history. Each desktop uses its own rootless Podman container, home, workspace, X server and DBus session. Up to two desktops can run at once, each with a PID limit. CPU, RAM and swap share the host’s available capacity. Accessible Linux DRM render nodes are passed through for compatible GPU workloads. The build supervisor's memory cap applies during compilation.

Human takeover is enforced server-side. When a controller lease expires, the manager releases held input and keeps agent input paused until a user selects **Return to agent**. Deleting a desktop removes the unit and container and retains its files.

Use the [deployment and pairing guide](../docs/setup.md) for prerequisites, rendered systemd units, migration, sign-in and validation. See the [MCP guide](../docs/mcp.md) for external tool configuration.

The optional `server/scripts/verify-live.mjs` checks desktop lifecycle, synchronization and takeover on a test host. It creates temporary identities and a fixture desktop and restarts the manager. Read the script before running it and set `D4C_HOSTNAME` to the certificate hostname.
