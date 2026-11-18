# D4C workspace manager

The manager owns workspace SQLite state, desktop inventory and bindings, drafts,
selection, activity and notification/read state. OpenClaw owns transcripts, active
runs, models, MCP tools and schedules. Closing a client never cancels server work.

Follow the [complete deployment guide](../docs/setup.md#3-persistent-workspace-and-desktops).
Deployment templates under `deploy/` must be rendered for your host with
`scripts/configure-server.py`; they contain no personal server address or fixed UID.

- Dedicated `d4c` account; rootless Podman and a user systemd instance with lingering.
- TLS manager on a private interface, normally port 8443.
- Client public keys approved separately using `vox-d4c-server --list-devices` and
  `vox-d4c-server --approve DEVICE_ID` as the manager account.
- Private Unix IPC socket for the locally installed OpenClaw desktop plugin.
  Only the configured `observerUid` may submit observer events. Filesystem access
  to the socket is also required; its group must contain the Gateway account.
- `D4C_SOCKET` overrides the plugin's default `/run/d4c/manager.sock`.
- Each desktop has its own rootless container, home, workspace, X server, runtime
  and DBus session. The supplied Arch/XFCE image includes a browser and fonts.

Two desktops may run at once; default resources per desktop are 1 GiB memory,
no additional swap, one CPU and a PID limit. Frames use one latest-frame slot.
Human takeover is enforced by the server, across clients and agent tools. Losing
a controller lease releases held input and leaves agent input paused until an
explicit Return to agent. Deleting a desktop removes its unit/container but
retains files; file removal is a separate operation.

The OpenClaw plugin uses session-scoped desktop tools, an observer and a durable
retry outbox. Screenshot tools use OpenClaw's direct route to preserve image
blocks; saved private screenshot files have bounded retention. Attaching a
desktop gives subsequent turns its control context without per-session package
installation. A screenshot tool result is distinct from sending its image as a
chat attachment.

The live verification script is optional and intrusive: it creates fixture
identities and a desktop and restarts the manager. Read it before use, run only
on an owned test deployment, and supply `D4C_HOSTNAME`. It is not an installer.
See [MCP setup](../docs/mcp.md) for the independent idle queue service.

## Explicit desktop memory policy

Administrators may set `D4C_DESKTOP_MEMORY_OVERRIDES` to a JSON object mapping
existing desktop UUIDs to memory in MiB, for example:

```sh
D4C_DESKTOP_MEMORY_OVERRIDES='{"00000000-0000-4000-8000-000000000001":1536}'
```

The manager validates UUIDs and limits (256–4096 MiB), admits increases only with
2 GiB host `MemAvailable` plus the increase in reserve, persists the admitted
budget, and updates a running container without restarting it. Memory and
memory-plus-swap limits are equal, allowing no additional container swap.
Rejected policies do not affect later lifecycle operations. Defaults and other
desktops stay unchanged. A higher limit provides headroom; it does not prevent
all browser OOMs or reserve that much physical memory.

For Codex desktop screenshot histories, see the separately installed,
[version-checked request budget adapter](codex-image-budget/README.md).
