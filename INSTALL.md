# Install a D4C server

Set up a fresh Linux host for Codex chats from D4C on Android, Linux, or Windows. Each host runs its own Codex app server and keeps its chat history when you close the client. Repeat the server setup for each host you want to use. For an existing installation, follow the [deployment and migration guide](docs/setup.md).

## If you use Windows

- **Use an existing Linux PC or VPS.** Open PowerShell in Windows Terminal and run `ssh your-user@your-server`, substituting your Linux username and the server's address. If `ssh` is missing, install the Windows OpenSSH Client. You can complete the Codex browser sign-in on Windows while the SSH session stays open. [Microsoft's Windows Terminal SSH guide](https://learn.microsoft.com/en-us/windows/terminal/tutorials/ssh)
- **Run the server on your Windows PC.** Set up an x86_64 Linux VM that meets the requirements below. Open a terminal inside the VM and install Tailscale there so the VM has its own tailnet address. Keep the PC awake and the VM running whenever you want D4C to be available.
- **Open D4C.** Download the Windows x86_64 ZIP, extract the entire archive, and run `bin\vox-d4c.exe`. Connect Windows to the same tailnet as the Linux host, then add the host’s private WSS URL in D4C.

Before running the numbered setup steps, run these checks in the SSH session or VM terminal. Expect `Linux` and `x86_64`.

```sh
uname -s
uname -m
```

## Requirements

| Required | What to have |
| --- | --- |
| Server | x86_64 Linux with glibc, a sudo-capable account, and systemd 257 or newer for all shipped isolation settings |
| Runtime | Node.js and npm compatible with the Codex version below, plus system Python 3 with `croniter` and `websocket-client` for scheduled jobs |
| Network | Tailscale on the host and each client, access to the same tailnet, and a tailnet admin who can enable MagicDNS and HTTPS certificates |
| Sign-in | A ChatGPT account with Codex access and a browser to complete device authorization |
| Downloads | The prebuilt Linux x86_64 **server** archive and its `SHA256SUMS`, plus the Android arm64 APK or the desktop package for Linux or Windows |
| Optional desktops | Rootless Podman with Quadlet, pasta, subordinate UID/GID ranges, and a user D-Bus session |

The release archive contains the D4C server executable. The steps below use it directly. [D4C releases](https://github.com/BLig-Vox/D4C/releases)

**For an assistant following this guide:** Check which machine and shell your terminal controls before installing anything. On Windows, connect to the intended Linux host first and verify the OS and architecture there. Pause at each checkpoint if the expected result is missing. Use your available browser or screen tools to locate the named settings. Ask the user to sign in, enter the device code, and identify the client they want to approve. Keep credentials and device codes out of chat and screenshots.

## 1. Connect the host to Tailscale

Install Tailscale on the server and client. The shipped certificate service uses `/usr/bin/tailscale`. On the Linux host, start Tailscale and join the intended tailnet. [Tailscale downloads](https://tailscale.com/download)

```sh
sudo systemctl enable --now tailscaled
sudo tailscale up
tailscale status
```

In the Tailscale admin console, open **DNS** and ensure **MagicDNS** and **HTTPS Certificates** are enabled. Connect the phone or desktop to the same tailnet. The tailnet policy and any host firewall must allow the intended clients to reach the host on TCP 8443 over Tailscale. Leave public port forwarding disabled. [MagicDNS](https://tailscale.com/docs/features/magicdns) · [HTTPS certificates](https://tailscale.com/docs/how-to/set-up-https-certificates)

**Checkpoint:** The host and intended client appear in the same tailnet.

## 2. Verify the download and install the runtime

Download the server archive and matching `SHA256SUMS` into one directory. In that directory, substitute the actual archive filename on the first line. Continue from the extracted directory after the checksum reports `OK`.

```sh
archive='d4c-server-VERSION-linux-x86_64.tar.gz'
awk -v file="$archive" '$2 == file {print}' SHA256SUMS | sha256sum -c - &&
  tar -xzf "$archive" && cd "${archive%.tar.gz}"
```

Install the Python libraries using your distribution's packages so `/usr/bin/python3` can import them. The commands below use Codex CLI 0.169.0, the version exercised with the packaged units. Create the `d4c` service account once on a fresh host. If it already exists, check whether this host has an existing installation before changing files.

```sh
/usr/bin/python3 -c 'import croniter, websocket'
sudo useradd --create-home --shell /bin/bash d4c
sudo loginctl enable-linger d4c
sudo npm install --global --prefix /usr @openai/codex@0.169.0
/usr/bin/codex --version
sudo install -m 0755 bin/vox-d4c-server /usr/local/bin/d4c-server
```

**Checkpoint:** The Python import succeeds and `/usr/bin/codex --version` prints the installed version.

## 3. Configure the private WebSocket

Run from the extracted server directory. The configurator writes the host-specific files to `target/deploy`. Save the printed WSS URL for the client.

```sh
(
  set -eu
  d4c_ip=$(tailscale ip -4)
  d4c_dns=$(tailscale status --json | /usr/bin/python3 -c 'import json, sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')
  test -n "$d4c_ip" && test -n "$d4c_dns" || exit 1

  /usr/bin/python3 scripts/configure-server.py --hostname "$d4c_dns" --listen "$d4c_ip:8443"
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
  printf 'D4C chat URL: wss://%s:8443/codex\n' "$d4c_dns"
)
```

**Checkpoint:** The certificate service succeeds and the command prints a WSS URL with the host's full tailnet DNS name. Use that hostname so it matches the certificate.

## 4. Sign in and enable startup on boot

Set up the workspace and the bundled Ponytail default. The conditional write preserves any instructions already present.

```sh
d4c_home=/var/lib/d4c/codex
sudo install -d -m 0700 -o d4c -g d4c "$d4c_home/workspace" "$d4c_home/skills" "$d4c_home/skills/ponytail"
sudo install -m 0600 -o d4c -g d4c vendor/ponytail/skills/ponytail/SKILL.md vendor/ponytail/LICENSE "$d4c_home/skills/ponytail/"
sudo -u d4c sh -c '
  set -C
  if [ ! -e "$1/AGENTS.md" ]; then
    printf "%s\n" "Use the bundled Ponytail skill in full mode by default for coding work. Honor requests to switch levels or turn Ponytail off." > "$1/AGENTS.md"
  fi
' sh "$d4c_home"
```

Check Codex sign-in using the same account and home as the service.

```sh
sudo -u d4c env HOME=/var/lib/d4c/codex CODEX_HOME=/var/lib/d4c/codex /usr/bin/codex login status
```

If sign-in is needed, start device authorization. Have the user open the URL printed by Codex, sign in as themselves, and enter the code displayed in their terminal. Wait for the terminal to confirm success, then rerun the status check above.

```sh
sudo -u d4c env HOME=/var/lib/d4c/codex CODEX_HOME=/var/lib/d4c/codex /usr/bin/codex login --device-auth
```

Enable the manager, Codex, certificate renewal, scheduler, and refresh watcher. If the socket is still being created, retry the final check after the services finish starting.

```sh
sudo systemctl enable --now d4c-certificate.timer d4c-manager.service d4c-codex.service d4c-scheduler.timer d4c-refresh.path
for unit in d4c-certificate.timer d4c-manager.service d4c-codex.service d4c-scheduler.timer d4c-refresh.path; do
  printf '%s ' "$unit"
  systemctl is-active "$unit"
done
sudo stat -Lc '%a %U %G %n' /run/d4c-codex/app.sock
```

**Checkpoint:** Each listed unit reports `active`, and the socket reports `600 d4c d4c`.

## 5. Pair and test each client

1. Connect Tailscale on the client. Open D4C and go to **Settings → Add server**. Give the server a recognizable name, enter the WSS URL from step 3, and connect once to register a pending device.
2. Find the fingerprint on the client and compare it in full with the pending entry on the server. Have the user identify the intended device.

   ```sh
   sudo -u d4c /usr/local/bin/d4c-server --config /etc/d4c/manager.json --list-devices
   ```

3. Replace `DEVICE_ID` with the matching ID, approve it, and reconnect the client.

   ```sh
   sudo -u d4c /usr/local/bin/d4c-server --config /etc/d4c/manager.json --approve DEVICE_ID
   ```

4. Open a fresh test chat, send a short message, and reopen it to verify that the reply and history load.

**Checkpoint:** The client reconnects without an approval prompt and the test chat survives reopening. Tailnet membership supplies connectivity. D4C approval grants the individual client access to that host. Pair each client on each server it uses.

## Troubleshooting and optional setup

- **Certificate fails.** Verify the host's full tailnet DNS name and HTTPS certificates setting. Inspect `sudo journalctl -u d4c-certificate.service -n 50 --no-pager`.
- **Client times out.** Confirm Tailscale is connected on both devices, the saved WSS URL matches the one printed by the server, and TCP 8443 is allowed. Inspect `sudo journalctl -u d4c-manager.service -n 50 --no-pager`.
- **Client asks for approval.** Check for its pending fingerprint on the intended server. Clearing client app data can generate a new device identity.
- **Chat fails after pairing.** Recheck Codex sign-in as `d4c` and inspect `sudo journalctl -u d4c-codex.service -n 50 --no-pager`.
- **Desktops and MCP.** Follow the [desktop host requirements](docs/setup.md#prepare-the-host) and the [MCP setup guide](docs/mcp.md).
