<!-- d4c-live-mcp:start -->
## D4C MCP connections
For a request to connect/configure/remove MCP, read `skills/connect-mcp/SKILL.md` and use its persistent queue helper. OpenClaw already has a built-in MCP client. Queued changes apply only after active chat turns end; end the requesting turn instead of polling inside it. Inspect the job receipt on the next message before claiming success. Never restart Gateway or patch MCP config inside the running model turn. Keep credentials out of chat and preserve existing connections. Authorized native MCP tools remain usable on subsequent turns after a verified job applies.
<!-- d4c-live-mcp:end -->
