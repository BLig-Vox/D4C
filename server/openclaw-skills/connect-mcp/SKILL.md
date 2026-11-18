---
name: connect-mcp
description: Connect, update or remove MCP servers in D4C without restarting Gateway. Uses persistent checked jobs after active chat turns finish.
---

# Connect MCP

OpenClaw has a built-in MCP client; no community client plugin is needed. Use the installed queue helper for user-requested MCP changes. It stores private jobs and an independent systemd worker applies them only after active chat turns finish. This avoids replacing a runtime underneath its own tool call.

1. Inspect current connections and prior jobs:
   `python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py list`
   `python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py status`
2. Queue the requested HTTPS endpoint:
   `python3 ~/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py connect NAME https://host/mcp`
   Optional `--transport sse` for legacy SSE, repeated `--allow RAW_TOOL_NAME` for an intentional tool filter. Names are case-sensitive letters/numbers/underscores/hyphens, up to64 characters.
3. A queued receipt is not a connection result. **End this turn** with a brief queued status; do not keep polling or waiting inside the same turn, because the worker is waiting for active turns to end. The independent worker continues if the phone/desktop closes. Next message: inspect `status JOB_ID`; only state `applied` confirms verified discovery and an applied configuration revision. If queued, allow this turn to finish; do not create duplicate jobs or bypass the idle guard.
4. When applied, use native MCP discovery on the next turn. A read-only tool call is the final proof of native access when authorized. Financial writes/deletions and other tool effects still follow normal user authorization and tool permissions.
5. Queue a removal with `disconnect NAME`; check its job result the same way.

## Authentication

Use `--bearer-file /absolute/private/token-file` for an already provisioned owner-only server-side file. The queue takes a private copy for durable processing; it never prints the token. The worker checks authentication and tools before publishing, stores only an environment placeholder in config, and removes the queued credential copy when finished. Do not ask for secrets in chat. Obtain/create credentials through an authorized protected browser/credential flow when available; report missing credentials plainly otherwise. Do not silently reuse another service's key.

Existing bearer headers are preserved on a connection refresh without `--bearer-file`. OAuth uses OpenClaw's native Settings → MCP sign-in at the Gateway's own HTTPS address; this static bearer worker does not bypass OAuth or impersonate a logged-in browser.

## Failure and recovery

An auth/network/discovery failure leaves the requested connection change unapplied. `status` gives a safe reason. Correct the endpoint/credential and queue a new request only after inspecting the failed result. The worker retries Gateway transport failures at most3 times, handles revision conflicts without clobbering other config, and can attempt one supported idle Codex-plugin reload for a valid saved/applied revision mismatch. It never restarts Gateway, edits databases or runs broad doctor repairs.

Never use CLI `openclaw mcp add/set/configure/probe/login/reload` against the live Gateway: those commands require offline ownership or affect only the CLI's own runtime. Do not patch MCP config directly inside this running model turn. Do not install a client plugin, change models/auth policies, disable TLS verification or weaken host permissions to make a connection work. For unsupported transports or configuration layouts, report the specific limitation rather than claiming a successful connection.
