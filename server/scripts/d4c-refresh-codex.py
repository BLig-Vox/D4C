#!/usr/bin/env python3
"""Reload native MCP configuration and refresh skill discovery on the running service."""
import json
import socket
from contextlib import closing

from websocket import create_connection


def refresh():
    with closing(socket.socket(socket.AF_UNIX)) as stream:
        stream.settimeout(10)
        stream.connect("/run/d4c-codex/app.sock")
        with closing(create_connection("ws://localhost/", socket=stream, timeout=15)) as ws:
            def call(request_id, method, params):
                ws.send(json.dumps({"id": request_id, "method": method, "params": params}))
                while True:
                    response = json.loads(ws.recv())
                    if response.get("id") == request_id:
                        if "error" in response:
                            raise RuntimeError(f"{method} failed with code {response['error']['code']}")
                        return

            call(1, "initialize", {"clientInfo": {"name": "d4c_refresh", "version": "1"},
                                   "capabilities": {"experimentalApi": True}})
            ws.send(json.dumps({"method": "initialized"}))
            call(2, "config/mcpServer/reload", {})
            call(3, "skills/list", {"forceReload": True})
    print("Codex accepted the MCP refresh and refreshed skill discovery")


if __name__ == "__main__":
    refresh()
