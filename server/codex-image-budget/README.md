# Codex screenshot request budget

A local, version-checked adapter for `@openclaw/codex` **2026.11.5**. Accumulated
inline screenshots can exceed the inference relay's 32 MiB request limit before
token-based compaction runs. Native compaction can then fail for the same reason.

`guard.mjs` projects outgoing requests without changing stored transcripts or PNGs.
It recognizes D4C screenshot tool results by their saved-file annotation, keeps
the newest observation and up to four recent images within a preferred 4 MiB
image budget, and replaces older screenshot pixels with an explicit text marker.
Saved-file references, other tool output, user attachments and instructions remain.
The newest observation is retained even if it exceeds that preferred budget;
the original 32 MiB outgoing cap still applies. This is not protection against
arbitrary oversized text or user uploads.

Both HTTP and WebSocket inference share the projection, including native
compaction. Local inbound bodies are bounded at 64 MiB to allow the projection;
the upstream request limit remains 32 MiB. Logs contain counts and byte totals,
not screenshots or message contents.

## Install

Keep these files together on the Gateway host. Discover the actual installed
`@openclaw/codex` package directory; do not assume a particular npm project hash.
Back up the private session state before recovery, and wait for active turns to
finish before restarting the Gateway.

```sh
python3 install.py --plugin-root /path/to/node_modules/@openclaw/codex
python3 install.py --plugin-root /path/to/node_modules/@openclaw/codex --apply
systemctl --user restart openclaw-gateway
```

The installer verifies the version, known source anchors and JavaScript syntax.
It preserves original modules as `*.mjs.d4c-original`, writes a hash receipt and
verifies those hashes on repeat runs. It refuses unsupported layouts. A partial
or changed installation requires review rather than blind replacement.

For an affected conversation, use the supported native recovery operation after
activation, replacing the placeholder with its session key:

```sh
openclaw sessions compact SESSION_KEY --timeout 240000 --json
```

Do not edit the native rollout or delete history to make compaction fit.

## Maintenance and rollback

The adapter persists across Gateway restarts. **An upstream package upgrade can
replace it or change the API; revalidate compatibility after every upgrade.**
This is a local patch, not a supported upstream configuration switch or a promise
that all future versions are compatible.

To roll back, stop the Gateway while idle, restore the dispatch and proxy
`.mjs.d4c-original` files to their corresponding `.mjs` paths, remove
`d4c-image-budget.mjs` and `d4c-image-budget-receipt.json`, then restart the Gateway.
Large uncompactable image histories may fail again without the adapter.

Background: [OpenClaw Codex troubleshooting](https://docs.openclaw.ai/plugins/codex-harness/troubleshooting).
