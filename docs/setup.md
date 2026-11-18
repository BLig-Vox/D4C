# Server setup

This guide targets Linux with systemd, rootless Podman and Tailscale. Examples use
`gateway.example.ts.net` and `100.64.0.10`; replace them with your own values.
OpenClaw 2026.11.5 and Codex CLI 0.169.0 were used during development. Check the
linked upstream guides before upgrading; their configuration interfaces can change.

## 1. OpenClaw and Codex authentication

Install a supported Node.js/npm release and OpenClaw on the server. Use a dedicated
Unix account for your Gateway and agent workspace. Sign into Tailscale on both the
server and every client device. Keep your D4C checkout available on the server.

```sh
npm install -g openclaw@2026.11.5
openclaw onboard --classic
```

In onboarding, choose your provider and model, complete its browser/device-code
sign-in, configure a local Gateway and workspace, and install the background service.
For OpenAI/Codex-backed operation, choose the offered OpenAI/device-code route and
authorized Codex runtime. If your installation requires the standalone Codex CLI,
install it according to its own official instructions and complete `codex login`
as the same Unix user. A D4C client token is a Gateway credential; it does not
replace provider authentication. Never send provider passwords or API keys in chat.

Confirm the service and available models from the server:

```sh
openclaw gateway status
openclaw models list
openclaw devices list
```

Use `openclaw configure` to adjust an existing installation. D4C discovers the
Gateway's model catalogue; it does not carry a fixed model allowlist. The Gateway's
provider access, model policy and Codex runtime determine which models work.

Configure managed private ingress during onboarding/configuration with these
Gateway settings (merge with your existing configuration):

```json
{
  "gateway": {
    "bind": "loopback",
    "tailscale": { "mode": "serve" }
  }
}
```

Keep Gateway authentication enabled. Give the Gateway account the Tailscale
operator privileges required by your deployment. HTTPS certificates/Serve must be
enabled in the tailnet. Use OpenClaw's managed Serve setup so its listener owns the
route; do not overwrite another service's existing route. An alternative trusted
TLS reverse proxy requires the upstream trusted-proxy settings and credentials.
D4C requires `wss://`; a bare tailnet IP with plain `ws://` is not accepted.

See [OpenClaw getting started](https://docs.openclaw.ai/start/getting-started) and
[managed Tailscale ingress](https://docs.openclaw.ai/gateway/tailscale) for the
complete authentication and service setup.

## 2. Pair D4C

In D4C, open Settings, add `wss://gateway.example.ts.net`, save and switch to it.
If token authentication is required, enter the Gateway token in the password field.
It stays in memory for the current connection; server URLs never contain tokens.
Then, as the Gateway user:

```sh
openclaw devices list
openclaw devices approve REQUEST_ID
```

Approve only the request belonging to your device. Reconnect in D4C. Requested
operator scopes must be approved by your Gateway; scheduling writes need
`operator.admin` in the development baseline. Pair each client independently.
No private identity file should be copied between desktop and phone.

Chat/transcript persistence is already server-side through OpenClaw. Continue with
the manager for shared drafts, notification/read state and persistent desktops.

## 3. Persistent workspace and desktops

Run administrative commands on the **server**, not on the Android device. The
manager and OpenClaw Gateway can live on the same host. For the first alpha, the
desktop plugin communicates over a local Unix socket, so a remote manager needs
its own colocated Gateway/plugin; there is no cross-host raw IPC tunnel.

Install build tools, rootless Podman (with Quadlet and pasta), uidmap, dbus-user-session,
Python 3 and Tailscale using your distribution's packages. Create the dedicated
manager account and persistent user instance:

```sh
sudo useradd --create-home --shell /bin/bash d4c
sudo loginctl enable-linger d4c
```

Check that `/etc/subuid` and `/etc/subgid` contain non-overlapping ranges for `d4c`.
Its user systemd instance and rootless Podman must work before starting the manager.
Build the binaries through the memory guard:

```sh
python3 scripts/build-limited.py --limit-mb 2000 -- \
  cargo build --manifest-path server/Cargo.toml --locked -j 1
sudo install -m 0755 server/target/debug/vox-d4c-server /usr/local/bin/d4c-server
```

Build the desktop image **as d4c**, under its rootless Podman store. Run from this
checkout, which must be readable by that account:

```sh
D4C_UID=$(id -u d4c)
sudo -u d4c env XDG_RUNTIME_DIR=/run/user/"$D4C_UID" \
  DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/"$D4C_UID"/bus \
  podman build -t localhost/d4c-arch-xfce:1 server/desktop
```

Package installation/image builds also use memory; perform them when your server
has sufficient headroom. Rust's build guard does not cap an unrelated Podman build.

Render templates using the Unix user that actually runs OpenClaw:

```sh
python3 scripts/configure-server.py \
  --hostname gateway.example.ts.net \
  --listen 100.64.0.10:8443 \
  --openclaw-user YOUR_GATEWAY_USER
```

The renderer discovers the `d4c` UID/home and the Gateway UID. Review
`target/deploy/manager.json` and the generated services. Default paths are
`/etc/d4c`, `/var/lib/d4c` and `/run/d4c`; edit generated files together if you
choose other paths. `hostname` must match the manager TLS certificate. Only
loopback and Tailscale listen addresses are accepted.

```sh
sudo install -d -m 0750 -o root -g d4c /etc/d4c /etc/d4c/tls
sudo install -d -m 0700 -o d4c -g d4c /var/lib/d4c
sudo install -m 0640 -o root -g d4c target/deploy/manager.json /etc/d4c/manager.json
sudo install -m 0644 target/deploy/d4c-*.service target/deploy/d4c-*.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl start d4c-certificate.service
sudo systemctl enable --now d4c-certificate.timer d4c-manager.service
```

The certificate job obtains a tailnet certificate, grants the manager group read
access to its key, and renewal restarts only the manager. Restrict tailnet ACLs to
the intended devices. The supplied systemd service caps manager resources and
enables the dedicated user's bus; no machine-specific numeric UID is shipped.

### Pair the workspace service

D4C defaults to the same DNS hostname on port 8443. Use Settings → Workspace server
to save a different secure URL for the selected Gateway. Save and reconnect; each
client stores this override locally. Pair it separately on the manager:

```sh
sudo -u d4c /usr/local/bin/d4c-server --config /etc/d4c/manager.json --list-devices
sudo -u d4c /usr/local/bin/d4c-server --config /etc/d4c/manager.json --approve DEVICE_ID
```

The fingerprint comes from D4C's connection status. The approval command and the
running manager must use the same configuration/database. Open the chat workspace
panel to create or attach a desktop. `/vnc connect NAME` binds it to that chat.
Tap the floating monitor bubble to open it, drag the bubble freely, and minimize
with the viewer's minus button. Take over pauses agent input server-side; explicitly
Return to agent when finished. Closing/minimizing a viewer keeps its desktop running.

### Connect OpenClaw's desktop tools

Allow the Gateway's Unix account to access the manager's private socket:

```sh
sudo usermod -aG d4c YOUR_GATEWAY_USER
```

A new login/service process must acquire the group membership. Do this during
initial setup before starting the Gateway, or arrange a planned Gateway restart
when no work is active. Do not make the socket world-writable. The manager's
`observerUid` must be the Gateway account's actual UID; the renderer sets it.

As the Gateway account, install the local plugin:

```sh
openclaw plugins install --force /absolute/path/to/D4C/server/openclaw-plugin
openclaw plugins enable d4c-desktops
```

Use upstream's supported plugin activation/reload procedure, outside an active
agent turn. If you changed the socket path, set `D4C_SOCKET` in the Gateway service
environment before activating. `OPENCLAW_STATE_DIR` controls plugin screenshot/outbox
storage. Agent tool policy must permit the `desktop_*` tools; D4C cannot override
that policy. Bind a desktop before asking the assistant to inspect or control it.

## 4. Connect MCP services

Install and use the [idle MCP queue](mcp.md). It is separate from the desktop tools
plugin: it configures OpenClaw's native MCP registry and requires no client plugin.
Public/Bearer HTTPS Streamable HTTP and SSE are supported by its discovery step;
OAuth stays in the native account flow. Do not restart the Gateway for each new
MCP request, and do not claim a queued connection is applied.

## 5. Check your deployment

- Confirm a short chat streams and the expected model is available.
- Send a message from phone and confirm it appears on another paired client.
- Pair the manager, create a desktop and check screenshot delivery and takeover.
- Ask for a paused schedule and inspect it in the chat's Tasks panel before enabling it.
- In Settings, send a test notification and check Android's permission/channel settings.
- Connect an MCP with minimal permissions, check its applied receipt, then request
  an authorized read-only tool call.

The queue and server state survive client closure. Android background delivery
uses a visible foreground service and remains subject to battery policy, network
availability and force-stop. A failed remote endpoint still needs operator action;
see [MCP failure and recovery](mcp.md#failure-and-recovery).
