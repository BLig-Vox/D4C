# Set up a D4C server

D4C connects to a paired TLS manager. The manager forwards native chat traffic to a persistent Codex app server over a private Unix socket. The same manager serves workspace data and desktops. Install the server components once on each host, then pair each desktop or phone.

## Prepare the host

Use Linux with systemd, Tailscale, Python 3, Node.js/npm and Codex CLI. The service templates were exercised with systemd 262 and Codex CLI 0.169.0. Install the distribution packages for rootless Podman, Quadlet, pasta, subordinate UID/GID mappings and a user DBus session for desktops. Install Python's `croniter` and `websocket-client` for the scheduler. Check that the system Python can import both.

Enable HTTPS certificates for your tailnet, sign in with Tailscale on each device, and allow the intended clients to reach the server's private ports. Use the server's complete Tailscale DNS name for TLS.

Run the following from the unpacked server release or the repository root. Examples use GNU utilities and a shell with a dedicated `d4c` account. Create the account once on a fresh host.

```sh
sudo useradd --create-home --shell /bin/bash d4c
sudo loginctl enable-linger d4c
sudo npm install --global --prefix /usr @openai/codex@0.169.0
/usr/bin/codex --version
/usr/bin/python3 -c 'import croniter, websocket'
sudo install -m 0755 bin/vox-d4c-server /usr/local/bin/d4c-server
```

For a source checkout, build the manager with the project's memory guard and install the resulting binary in place of the release binary.

```sh
python3 scripts/build-limited.py --limit-mb 2000 -- cargo build --manifest-path server/Cargo.toml --locked -j 1
sudo install -m 0755 server/target/debug/vox-d4c-server /usr/local/bin/d4c-server
```

Check that the `d4c` account has non-overlapping ranges in `/etc/subuid` and `/etc/subgid`. Build the desktop image in that account's Podman store from a directory it can read.

```sh
d4c_uid=$(id -u d4c)
sudo -u d4c env HOME="$(getent passwd d4c | cut -d: -f6)" XDG_RUNTIME_DIR="/run/user/$d4c_uid" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$d4c_uid/bus" \
  podman build -t localhost/d4c-arch-xfce:1 server/desktop
```

## Install the manager, Codex and scheduler

Replace the example hostname and address with values for this host. These commands install a **fresh configuration**. For an existing manager, retain its database, TLS keys and other settings, back up its configuration, and merge the `codexSocket` setting from the generated file. Keep its device approvals and desktop data in place.

```sh
python3 scripts/configure-server.py --hostname gateway.example.ts.net --listen 100.64.0.10:8443
sudo install -d -m 0750 -o root -g d4c /etc/d4c /etc/d4c/tls
sudo install -d -m 0700 -o d4c -g d4c /var/lib/d4c /var/lib/d4c/codex /var/lib/d4c/uploads /var/lib/d4c/schedules
sudo install -m 0640 -o root -g d4c target/deploy/manager.json /etc/d4c/manager.json
sudo install -m 0644 target/deploy/*.service target/deploy/*.socket target/deploy/*.timer target/deploy/*.path /etc/systemd/system/
sudo install -m 0644 target/deploy/d4c-codex-tmpfiles.conf /etc/tmpfiles.d/d4c-codex.conf
sudo systemd-tmpfiles --create /etc/tmpfiles.d/d4c-codex.conf
sudo install -D -m 0755 server/scripts/d4c-scheduler.py /usr/local/libexec/d4c-scheduler
sudo install -D -m 0755 server/scripts/d4c-refresh-codex.py /usr/local/libexec/d4c-refresh-codex.py
sudo systemctl daemon-reload
sudo systemctl start d4c-certificate.service
```

Codex 0.169.0 creates a mode-0600 Unix socket under a private per-user directory in `/tmp` and links `/run/d4c-codex/app.sock` to it. The tmpfiles rule creates this directory on boot. The service shares this directory across its private mount namespace. Its work directory is `/var/lib/d4c/codex/workspace`. Uploaded attachments appear read-only at `/var/lib/d4c/uploads`.

On systemd-resolved hosts, the unit mounts `/run/systemd/resolve` read-only inside its private `/run`. If a CLI chat succeeds but the service times out fetching models, check that `/etc/resolv.conf` resolves inside the service mount namespace after updating the unit and restarting it.

### Migrate an existing OpenClaw host

For a migration, complete this step **before** creating the new workspace or launching the Codex service. Wait for active work to finish, take the final snapshot while the legacy service is idle, then stop and disable its gateway and schedule workers. Use the paths for the actual gateway account. Each snapshot destination must be new. Keep the source data and verified snapshot available for recovery.

```sh
sudo python3 scripts/stage-codex.py /var/lib/d4c/migration/cutover-01 \
  --openclaw /path/to/gateway-account/.openclaw --workspace /var/lib/d4c/workspace.sqlite
sudo python3 scripts/activate-codex-home.py /var/lib/d4c/migration/cutover-01 \
  --workspace /path/to/gateway-account/.openclaw/workspace
```

Before starting Codex, import the native thread aliases into the manager and register the scoped D4C tools in existing rollout metadata. Keep the app server stopped while rewriting rollout metadata.

```sh
sudo -u d4c /usr/local/bin/d4c-server --import-aliases /var/lib/d4c/codex/migration-manifest.json
sudo -u d4c /usr/local/bin/d4c-server --migrate-codex-tools /var/lib/d4c/codex
```

The activation script verifies recorded native rollout hashes, copies the workspace, and rewrites thread paths to the new home. Only current D4C chat bindings appear in the sidebar after migration. Other imported threads, including old bindings and scheduled runs, retain their rollouts in `archived_sessions`. New chats appear normally. The optional `--migrate-openai-profile` flag imports an unambiguous OpenAI profile from the private snapshot. When authentication is missing or expired, sign in as described below. Retain the original provider and schedule data until you have verified conversations and scheduled tasks on the new host.

Import the archived jobs and run receipts before starting the timer. The final snapshot can contain credentials, so read it as its existing owner. Give the service account ownership of its new scheduler store. Jobs whose legacy target cannot run natively retain a migration error and stay disabled.

```sh
sudo /usr/local/libexec/d4c-scheduler import /var/lib/d4c/migration/cutover-01
sudo chown -R d4c:d4c /var/lib/d4c/schedules
```

### Set up the working directory, Ponytail and authentication

Install the bundled skill. On an existing installation, retain local skill changes and merge the default instruction into the existing `AGENTS.md`.

```sh
d4c_codex=/var/lib/d4c/codex
sudo install -d -m 0700 -o d4c -g d4c "$d4c_codex/workspace" "$d4c_codex/skills" "$d4c_codex/skills/ponytail"
sudo install -m 0600 -o d4c -g d4c vendor/ponytail/skills/ponytail/SKILL.md vendor/ponytail/LICENSE "$d4c_codex/skills/ponytail/"
printf '%s\n' 'Use the bundled Ponytail skill in full mode by default for coding work. Honor requests to switch levels or turn Ponytail off.' \
  | sudo -u d4c tee -a "$d4c_codex/AGENTS.md" >/dev/null
sudo -u d4c env HOME="$d4c_codex" CODEX_HOME="$d4c_codex" /usr/bin/codex login status
```

If the account needs authentication, run the following on the host and complete the device sign-in in a browser. Credentials belong to this host's Codex account. D4C clients authenticate with their device keys.

```sh
sudo -u d4c env HOME=/var/lib/d4c/codex CODEX_HOME=/var/lib/d4c/codex /usr/bin/codex login --device-auth
```

Start the persistent units. The scheduler timer runs once per host and owns its local job store.

```sh
sudo systemctl enable --now d4c-certificate.timer d4c-manager.service d4c-codex.service d4c-scheduler.timer d4c-refresh.path
sudo systemctl is-active d4c-manager.service d4c-codex.service d4c-scheduler.timer d4c-refresh.path
sudo stat -Lc '%a %U %G %n' /run/d4c-codex/app.sock
```

The `d4c-refresh.path` unit watches the Codex MCP configuration and skills directory. It invokes the native MCP reload and refreshes skill discovery. To request a refresh explicitly, run `sudo systemctl start d4c-refresh.service` and check its journal.

## Connect and pair devices

Use `wss://gateway.example.ts.net:8443/codex` for direct chat. The same TLS listener serves the workspace API at `/control` and desktop viewers at `/vnc/<desktop-id>`. The root path also serves paired chat. Native clients answer a nonce challenge before receiving any app-server traffic.

To keep a saved URL such as `wss://gateway.example.ts.net:8444/`, enable a socket relay for that port. For a saved HTTPS default port, use `443` in place of `8444`. Confirm that the port is free before enabling the relay. On a host with a legacy Tailscale Serve route, inspect `tailscale serve status` and retire only the chat handler on the chosen port. Keep unrelated HTTPS and VNC routes in place.

```sh
sudo systemctl enable --now d4c-chat@8444.socket
systemctl show d4c-chat@8444.socket -p BindToDevice
```

The socket is bound to `tailscale0` and forwards the encrypted connection to the manager. The manager presents its certificate and enforces the same device approval on both ports.

In D4C Settings, save and select the server URL. Attempt a connection, compare the client fingerprint with the pending device on the server, then approve that fingerprint and reconnect.

```sh
sudo -u d4c /usr/local/bin/d4c-server --config /etc/d4c/manager.json --list-devices
sudo -u d4c /usr/local/bin/d4c-server --config /etc/d4c/manager.json --approve DEVICE_ID
```

## Verify the deployment

1. Send a short chat, confirm a response, then reopen its thread from a second paired client. For a migration, open an existing conversation and confirm its history.
2. Open the workspace panel, create or attach a desktop, then open its viewer. Take over pauses agent input. Select **Return to agent** when finished. Closing the viewer leaves the desktop running.
3. Create a disabled schedule in Tasks, check the prompt and timing, then run it manually. Open its run history.
4. Check service logs with `journalctl -u d4c-manager -u d4c-codex -u d4c-scheduler`. If local history works but a turn fails at the model provider, check the Codex login for the service account.

For connected tools and authentication, see the [MCP guide](mcp.md).
