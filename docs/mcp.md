# MCP connections

D4C does not need a separate MCP-client plugin. OpenClaw owns the native registry
and tool runtime. The desktop plugin in this repository provides VNC tools; it
is separate from connecting third-party MCP servers.

## Install once per Gateway

Prerequisites: OpenClaw 2026.11.5, its npm package and dependencies, Node/npm,
Python 3, and a running user systemd Gateway. Run the following **as the Unix
user that owns the Gateway**, from your D4C checkout:

```sh
python3 scripts/install-mcp-queue.py
systemctl --user status d4c-mcp-queue.timer
```

The installer copies `connect-mcp` into your agent workspace, adds a marked
instruction block to its existing `AGENTS.md`, and enables a user timer. It
preserves unrelated workspace instructions. It does not restart the Gateway.
Enable lingering for this account if the user manager must survive logout:
`sudo loginctl enable-linger YOUR_GATEWAY_USER`.

Custom installations can specify:

```sh
python3 scripts/install-mcp-queue.py \
  --state-dir /path/to/openclaw-state \
  --workspace /path/to/agent-workspace \
  --openclaw-package-root /path/to/node_modules/openclaw \
  --gateway-unit openclaw-gateway.service
```

`OPENCLAW_CONFIG_PATH` is preserved in the worker if set during installation.
Use the actual agent workspace, not a client-side copy. Install the skill into
each workspace that should use it. One queue processes a Gateway's shared state.
The installer discovers the installed OpenClaw package and writes the worker's
PATH; reinstall after moving your Node/OpenClaw installation.

## Connect from chat

Ask: **“Connect the MCP named reports at https://reports.example/mcp.”**
The agent should use the skill, return a queued receipt, and finish its turn.
Ask on the next message: **“Check that MCP connection.”** A worker can only apply
when the Gateway reports all chat turns idle. Continuous active work delays it.
Queued is not connected. `applied` confirms discovery and publication; an actual
authorized tool call confirms your intended workflow.

The equivalent server commands, for the default workspace:

```sh
python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py connect reports https://reports.example/mcp
python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py status
python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py list
```

Use `--transport sse` for an SSE endpoint. Streamable HTTP is the default. The
URL must be HTTPS without an embedded username, password, query string or fragment.
Use the returned job ID after `status` to inspect one receipt. Custom workspaces
use their installed helper path instead.

## Bearer credentials

Provision the token on the Gateway as an owner-only regular file, without
pasting it into chat. For example, this prompts privately on the server:

```sh
install -d -m 700 ~/.config/d4c-secrets
python3 - <<'PY'
import getpass, os
from pathlib import Path
p = Path.home() / '.config/d4c-secrets/reports.token'
fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.fchmod(fd, 0o600)
with os.fdopen(fd, 'w') as f:
    f.write(getpass.getpass('MCP bearer token: ') + '\n')
PY
python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py connect reports https://reports.example/mcp \
  --bearer-file ~/.config/d4c-secrets/reports.token
```

The queue snapshots a private copy for durable processing. After successful
validation, the helper stores an environment reference in the native registry
and the value in the Gateway's private `.env`; it never returns it in a receipt.
The queued token copy is removed after terminal success/failure. Protect the
original file and remove it when no longer needed. Removing an MCP connection
does not revoke the remote token or purge historical `.env` variables: revoke
credentials at the provider and remove unused secret entries separately.

Existing authentication and unrelated headers are preserved when no new token is
supplied. Rotating a bearer removes differently cased Authorization headers to
avoid sending conflicting credentials. Names cannot silently move to a different
endpoint; remove/recreate deliberately or choose another name.

For **OAuth**, use the native OpenClaw MCP account sign-in flow. This helper does
not automate OAuth login or bypass consent. Custom `$include` layouts, unsupported
transports or auth schemes fail safely and need the native settings workflow.

## Remove and inspect

```sh
python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py disconnect reports
python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py status JOB_ID
```

Disconnect is also queued. It does not restart the Gateway or remove unrelated
connections. Jobs survive client closure and service restarts. Pending requests
are bounded; identical pending requests share a receipt.

## Failure and recovery

| Status | Meaning / action |
| --- | --- |
| queued | Waiting for idle chat turns, or a bounded transient retry |
| checking | Read-only initialization and tool listing; no target config write yet |
| recovering | One supported idle Codex plugin reload for an unapplied config revision |
| applying | Publishing the targeted registry update and checking saved/applied revisions |
| applied | Discovery and config publication confirmed; use tools on the next turn |
| failed | Read the redacted detail, correct URL/auth/policy, then submit a new request |

```sh
systemctl --user status d4c-mcp-queue.timer d4c-mcp-queue.service
journalctl --user -u d4c-mcp-queue.service --since '10 minutes ago'
```

The worker allows three transient update attempts and at most one idle Codex
plugin reload per job. It does not restart Gateway, install arbitrary plugins,
disable TLS, or widen permissions. A failed authentication/discovery check never
publishes the target connection. If publication is not confirmed, inspect native
saved/applied configuration before resubmitting; a response timeout may occur after
a write. MCP access still depends on OpenClaw's tool policy and provider permissions.

This is a durable, bounded connection workflow, not a guarantee against remote
outages or upstream runtime defects. Idle checks do not lock new run admission;
OpenClaw remains responsible for its runtime lifecycle. Do not poll a queued job
inside the same agent turn that submitted it, patch MCP state directly in that
turn, or run offline-owning `openclaw mcp` mutation commands against a live Gateway.
A CLI-owned MCP runtime is distinct from the Gateway's runtime.

Upstream reference: [OpenClaw MCP documentation](https://docs.openclaw.ai/tools/mcp).
