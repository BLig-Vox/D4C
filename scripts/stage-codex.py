#!/usr/bin/env python3
"""Take a private, verified migration snapshot without stopping the serving host."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
import uuid


def snapshot_database(source, target):
    with contextlib.closing(sqlite3.connect(source.resolve().as_uri() + "?mode=ro", uri=True)) as reader:
        with contextlib.closing(sqlite3.connect(target)) as writer:
            reader.backup(writer, pages=256)
            if writer.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise ValueError(f"Invalid snapshot of {source.name}")


def rollouts(home):
    return {path.relative_to(home): (path.stat().st_size, path.stat().st_mtime_ns)
            for folder in ("sessions", "archived_sessions")
            for path in (home / folder).rglob("rollout-*.jsonl")}


def copy_rollouts(source, target):
    before = rollouts(source)
    records = {}
    for relative in sorted(before):
        dest = target / relative
        dest.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(source / relative, dest)
        digest = hashlib.sha256()
        with dest.open("rb") as data:
            head = json.loads(data.readline())
            if head["type"] != "session_meta":
                raise ValueError(f"Missing session metadata in {relative.name}")
            thread = str(uuid.UUID(head["payload"]["id"]))
            if thread in records:
                raise ValueError(f"Duplicate native thread {thread}")
            data.seek(0)
            for line in data:
                json.loads(line)
                digest.update(line)
        records[thread] = {"path": str(relative), "sha256": digest.hexdigest()}
    if before != rollouts(source):
        raise RuntimeError("Rollouts changed during staging. Retry after the active turn finishes.")
    return records, before


@contextlib.contextmanager
def app_server(home):
    # Use a fresh index. A copied state database can point back to the live rollout.
    env = dict(os.environ, CODEX_HOME=str(home))
    proc = subprocess.Popen(["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, cwd=home, env=env)
    sequence = 0

    def send(message):
        proc.stdin.write(json.dumps(message) + "\n")
        proc.stdin.flush()

    def call(method, params):
        nonlocal sequence
        sequence += 1
        send({"id": sequence, "method": method, "params": params})
        for line in proc.stdout:
            message = json.loads(line)
            if message.get("id") == sequence:
                if "error" in message:
                    raise RuntimeError(f"{method} failed: {message['error'].get('code')}")
                return message["result"]
        raise RuntimeError("Codex stopped during verification")

    try:
        call("initialize", {"clientInfo": {"name": "d4c_migration", "version": "1"}, "capabilities": {"experimentalApi": True}})
        send({"method": "initialized"})
        yield call
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        proc.stdin.close()
        proc.stdout.close()


def import_history(home, records, dbdir, bindings):
    with app_server(home) as call:
        for thread, record in records.items():
            info = call("thread/read", {"threadId": thread, "includeTurns": False})["thread"]
            if info["id"] != thread or Path(info["path"]).resolve() != home / record["path"]:
                raise ValueError(f"Codex resolved {thread} outside its snapshot")
        with contextlib.closing(sqlite3.connect(dbdir / "openclaw-agent.sqlite")) as db:
            for row in bindings.values():
                if row["nativeVerified"]:
                    continue
                if row["threadId"]:
                    raise ValueError("Cannot replace a binding whose native rollout is missing")
                items = []
                for raw, compressed in db.execute("""
                    SELECT t.event_json,t.event_zstd FROM session_transcript_active_events a
                    JOIN transcript_events t ON t.session_id=a.session_id AND t.seq=a.event_seq
                    WHERE a.session_id=? ORDER BY a.active_position
                """, (row["sessionId"],)):
                    if raw is None:
                        raw = subprocess.run(["zstd", "-dc"], input=compressed,
                                             stdout=subprocess.PIPE, check=True).stdout
                    event = json.loads(raw)
                    if event["type"] == "session":
                        continue
                    message = event["message"]
                    role = message["role"]
                    if role not in ("user", "assistant"):
                        raise ValueError("Unsupported legacy history role; original retained")
                    content = message["content"]
                    if isinstance(content, str):
                        content = [{"type": "text", "text": content}]
                    if any(block["type"] != "text" for block in content):
                        raise ValueError("Unsupported legacy history block; original retained")
                    items.append({"type": "message", "role": role, "content": [
                        {"type": "input_text" if role == "user" else "output_text", "text": block["text"]}
                        for block in content]})
                if not items:
                    row["empty"] = True
                    continue
                cwd = db.execute(
                    "SELECT json_extract(event_json,'$.cwd') FROM transcript_events "
                    "WHERE session_id=? AND navigation_type='session' ORDER BY seq LIMIT 1",
                    (row["sessionId"],)).fetchone()
                if not cwd or not cwd[0] or not Path(cwd[0]).is_absolute() or not Path(cwd[0]).is_dir():
                    raise ValueError("Cannot resolve original working directory for legacy session")
                thread = call("thread/start", {"cwd": cwd[0], "sandbox": "read-only"})["thread"]["id"]
                call("thread/inject_items", {"threadId": thread, "items": items})
                # Require persisted records, not only an acknowledgment from the process.
                matches = list((home / "sessions").rglob(f"rollout-*-{thread}.jsonl"))
                if len(matches) != 1:
                    raise ValueError("Imported thread has no unique persisted rollout")
                with matches[0].open() as data:
                    saved = [e["payload"] for line in data if (e := json.loads(line))["type"] == "response_item"]
                if [(i["role"], i["content"]) for i in saved[-len(items):]] != [
                        (i["role"], i["content"]) for i in items]:
                    raise ValueError("Imported transcript differs from source")
                row.update(threadId=thread, importedMessages=len(items))
    # Indexes generated during verification reference the temporary directory.
    # Keep only portable rollouts here. The original SQLite snapshots live separately.
    for path in home.iterdir():
        if path.name not in ("sessions", "archived_sessions"):
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()


def session_map(dbdir, records):
    with contextlib.closing(sqlite3.connect(dbdir / "openclaw.sqlite")) as controller:
        bindings = {}
        for (value,) in controller.execute(
            "SELECT value_json FROM plugin_state_entries WHERE plugin_id='codex' "
            "AND namespace='app-server-thread-bindings'"
        ):
            entry = json.loads(value)
            if entry.get("state") != "active":
                continue
            session = entry["sessionId"]
            thread = entry["binding"]["threadId"]
            if session in bindings and bindings[session] != thread:
                raise ValueError(f"Ambiguous thread binding for {session}")
            bindings[session] = thread
    with contextlib.closing(sqlite3.connect(dbdir / "openclaw-agent.sqlite")) as agent:
        result = {}
        for key, session in agent.execute("SELECT session_key,current_session_id FROM session_nodes"):
            thread = bindings.get(session)
            result[key] = {"sessionId": session, "threadId": thread,
                           "nativeVerified": thread in records}
        return result


def stage(claw, workspace, dest):
    if dest.exists():
        raise FileExistsError(dest)
    dest.parent.mkdir(parents=True, exist_ok=True)
    agent = claw / "agents/main/agent"
    native = agent / "codex-home"
    if not native.is_dir():
        raise FileNotFoundError(native)
    with tempfile.TemporaryDirectory(dir=dest.parent) as scratch:
        out = Path(scratch) / "snapshot"
        out.mkdir()
        home = out / "native"
        home.mkdir()
        records, before = copy_rollouts(native, home)
        dbdir = out / "databases"
        dbdir.mkdir()
        databases = [agent / "openclaw-agent.sqlite", claw / "state/openclaw.sqlite", workspace]
        databases.extend(sorted(p for p in native.glob("*.sqlite") if not p.name.startswith("logs_")))
        for source in databases:
            snapshot_database(source, dbdir / source.name)
        bindings = session_map(dbdir, records)
        import_history(home, records, dbdir, bindings)
        if before != rollouts(native):
            raise RuntimeError("A turn changed during verification. Retry when idle.")
        (out / "manifest.json").write_text(json.dumps({"version": 1, "threads": records, "sessions": bindings,
                "databases": [p.name for p in databases]}, indent=2) + "\n")
        out.rename(dest)
    matched = sum(row["nativeVerified"] for row in bindings.values())
    print(f"Matched {matched}/{len(bindings)} chat bindings.")
    print(f"Verified {len(records)} native thread IDs and {len(databases)} database snapshots.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--openclaw", type=Path, default=Path.home() / ".openclaw")
    parser.add_argument("--workspace", type=Path, default=Path("/var/lib/d4c/workspace.sqlite"))
    args = parser.parse_args()
    os.umask(0o077)
    stage(args.openclaw.resolve(), args.workspace.resolve(), args.destination.resolve())
