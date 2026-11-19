#!/usr/bin/env python3
"""Install a verified, offline D4C migration snapshot into its dedicated Codex home."""
import argparse
import contextlib
import hashlib
import fcntl
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import sqlite3
import tempfile
import uuid


def copy_rollout(source, dest, old_workspace, workspace):
    # Preserve message content byte-for-byte. Only runtime metadata has relocated.
    dest.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as src, dest.open("xb") as out:
        for raw in src:
            event = json.loads(raw)
            if event["type"] not in ("session_meta", "turn_context"):
                out.write(raw)
                continue
            payload = event["payload"]
            if payload.get("cwd") == str(old_workspace):
                payload["cwd"] = str(workspace)
            if isinstance(payload.get("runtime_workspace_roots"), list):
                payload["runtime_workspace_roots"] = [
                    str(workspace) if x == str(old_workspace) else x for x in payload["runtime_workspace_roots"]]
            out.write(json.dumps(event, separators=(",", ":"), ensure_ascii=False).encode() + b"\n")


def install(args):
    home, snap, old = args.home.resolve(), args.snapshot.resolve(), args.workspace.resolve()
    account = pwd.getpwnam(args.user)
    manifest = json.loads((snap / "manifest.json").read_text())
    if not old.is_dir():
        raise ValueError("Original workspace is missing")
    home.mkdir(mode=0o700, parents=True, exist_ok=True)
    marker = home / "migration-manifest.json"
    if marker.exists():
        if json.loads(marker.read_text()) != manifest:
            raise ValueError("Another migration has already been activated")
        return
    workspace = home / "workspace"
    with tempfile.TemporaryDirectory(dir=home.parent) as temp:
        stage = Path(temp)
        shutil.copytree(old, stage / "workspace", symlinks=True)
        # Preserve original workspace instructions as reference. Legacy gateway
        # bootstrap instructions would otherwise run on every new native turn.
        instructions = stage / "workspace/AGENTS.md"
        if instructions.exists():
            instructions.rename(stage / "workspace/OPENCLAW-AGENTS.md")
        chat_threads = {row["threadId"] for key, row in manifest["sessions"].items()
                        if re.fullmatch(r"agent:[^:]+:d4c-[^:]+", key) and row.get("threadId")}
        installed_paths = {}
        for src in (snap / "native").rglob("rollout-*.jsonl"):
            rel = src.relative_to(snap / "native")
            with src.open("rb") as f:
                tid = str(uuid.UUID(json.loads(f.readline())["payload"]["id"]))
                record = manifest["threads"].get(tid)
                if record:
                    f.seek(0)
                    if hashlib.file_digest(f, "sha256").hexdigest() != record["sha256"]:
                        raise ValueError("Rollout hash mismatch")
            if rel.parts[0] == "sessions" and tid not in chat_threads:
                rel = Path("archived_sessions") / rel.name
            installed_paths[tid] = rel
            copy_rollout(src, stage / rel, old, workspace)
        for src in (snap / "databases").glob("*.sqlite"):
            if src.name.split("_")[0] in ("state", "thread", "queue", "memories", "goals"):
                shutil.copy2(src, stage / src.name)
        for path in stage.glob("state_*.sqlite"):
            with contextlib.closing(sqlite3.connect(path)) as db, db:
                for tid, rel in installed_paths.items():
                    db.execute("UPDATE threads SET rollout_path=?,cwd=CASE WHEN cwd=? THEN ? ELSE cwd END WHERE id=?",
                               (str(home / rel),str(old),str(workspace),tid))
                    if rel.parts[0] == "archived_sessions":
                        db.execute("UPDATE threads SET archived=1, archived_at=COALESCE(archived_at,unixepoch()) WHERE id=?", (tid,))
                with contextlib.closing(sqlite3.connect(snap / "databases/openclaw-agent.sqlite")) as agent:
                    for key, title in agent.execute("SELECT session_key,COALESCE(NULLIF(label,''),display_name) FROM session_nodes"):
                        row = manifest["sessions"].get(key,{})
                        if title and row.get("threadId"):
                            db.execute("UPDATE threads SET name=? WHERE id=?",(title,row["threadId"]))
        # Network MCP entries are carried over only when Codex can express the same transport.
        config = json.loads((old.parent / "openclaw.json").read_text()).get("mcp",{}).get("servers",{})
        lines = []
        for name, spec in config.items():
            if not spec.get("enabled", True):
                continue
            if not spec.get("url","").startswith("https://") or spec.get("transport") not in ("streamable-http","streamableHttp","http",None):
                raise ValueError(f"Review unsupported MCP transport for {name}")
            section = "mcp_servers." + json.dumps(name)
            lines.extend(["["+section+"]","url = "+json.dumps(spec["url"])])
            if spec.get("headers"):
                lines.append("["+section+".http_headers]")
                lines.extend(json.dumps(k)+" = "+json.dumps(v) for k,v in spec["headers"].items())
        if lines:
            (stage / "config.toml").write_text("\n".join(lines)+"\n")
        if args.migrate_openai_profile and not (home / "auth.json").exists():
            with contextlib.closing(sqlite3.connect(snap / "databases/openclaw-agent.sqlite")) as db:
                profiles = [a for row, in db.execute("SELECT store_json FROM auth_profile_store")
                            for a in json.loads(row).get("profiles",{}).values()
                            if a.get("provider") == "openai" and a.get("type") == "oauth"]
            if len(profiles) != 1:
                raise ValueError("No unambiguous OpenAI profile to migrate")
            p = profiles[0]
            tokens = {dest:p[src] for dest,src in (("access_token","access"),("id_token","idToken"),
                      ("refresh_token","refresh"),("account_id","accountId"))}
            (stage/"auth.json").write_text(json.dumps({"auth_mode":"chatgpt","tokens":tokens}))
        (stage / marker.name).write_text(json.dumps(manifest))
        entries = sorted(stage.iterdir(), key=lambda f: f.name == marker.name)
        # If interrupted between renames, only reuse entries owned by the same
        # staged manifest. Existing unrelated installations are never overwritten.
        journal = home / ".d4c-activation.json"
        digest = hashlib.sha256((snap / "manifest.json").read_bytes()).hexdigest()
        pending = {"snapshot":digest, "names":[f.name for f in entries]}
        resumed = journal.exists()
        if resumed and json.loads(journal.read_text())["snapshot"] != digest:
            raise ValueError("An interrupted migration must resume from its original snapshot")
        owned = set(json.loads(journal.read_text())["names"]) if resumed else set()
        for f in entries:
            if (home / f.name).exists() and f.name not in owned:
                raise ValueError(f"Existing {f.name} must be reconciled before activation")
        for f in stage.rglob("*"):
            os.chown(f,account.pw_uid,account.pw_gid,follow_symlinks=False)
        if not resumed:
            with journal.open("x") as file:
                json.dump(pending,file)
                file.flush()
                os.fsync(file.fileno())
        for f in entries:
            if not (home / f.name).exists():
                f.rename(home / f.name)
        journal.unlink()
    print(f"Activated {len(manifest['threads'])} native threads and {sum(bool(v.get('importedMessages')) for v in manifest['sessions'].values())} imported transcripts")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("snapshot",type=Path)
    p.add_argument("--home",type=Path,default=Path("/var/lib/d4c/codex"))
    p.add_argument("--workspace",type=Path,default=Path.home()/".openclaw/workspace")
    p.add_argument("--user",default="d4c")
    p.add_argument("--migrate-openai-profile",action="store_true")
    os.umask(0o077)
    args = p.parse_args()
    args.home.parent.mkdir(parents=True,exist_ok=True)
    with (args.home.parent / ".codex-activation.lock").open("a") as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        install(args)
