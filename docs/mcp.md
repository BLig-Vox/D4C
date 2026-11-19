# MCP in D4C

Each host has its own native Codex MCP configuration. In the packaged service,
`CODEX_HOME` is `/var/lib/d4c/codex`. Run these commands on the host where the
tools should execute, using the same service account and home.

```sh
sudo -u d4c env CODEX_HOME=/var/lib/d4c/codex codex mcp list
sudo -u d4c env CODEX_HOME=/var/lib/d4c/codex codex mcp add docs --url https://developers.openai.com/mcp
```

For a server that uses OAuth, run `codex mcp login NAME` with the same environment and complete its browser authorization. Keep credentials in the service account’s private storage.

Open an idle conversation in D4C and choose **Settings → Reload current session**.
Codex refreshes the MCP configuration and available skills. Ask it to perform a
small read-only operation with the new tool to verify the connection and account
permissions. Open conversations keep their thread IDs.

Install custom skills under `$CODEX_HOME/skills/NAME/SKILL.md`. The packaged
Ponytail skill supplies the default coding style. Desktop input tools resolve
the desktop attached to the requesting conversation. Taking over a desktop
pauses agent input until you return control.
