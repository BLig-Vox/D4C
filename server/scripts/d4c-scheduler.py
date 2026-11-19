#!/usr/bin/python3
"""D4C task store and timer. Dependencies: python-croniter, python-websocket-client."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import socket
import sqlite3
import sys
import time
import uuid
from zoneinfo import ZoneInfo

from croniter import croniter

ROOT = Path("/var/lib/d4c/schedules")
SOCKET = "/run/d4c-codex/app.sock"


def now():
    return time.time_ns() // 1_000_000


def uid():
    return str(uuid.uuid4())


def open_store(directory):
    db = sqlite3.connect(directory / "scheduler.sqlite3", timeout=5)
    db.row_factory = sqlite3.Row
    db.executescript("""
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY, owner TEXT, body TEXT NOT NULL, due INTEGER);
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY, job TEXT NOT NULL, owner TEXT, at INTEGER NOT NULL,
            phase TEXT NOT NULL, body TEXT NOT NULL, messages TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS run_order ON runs(at DESC);
        CREATE INDEX IF NOT EXISTS run_job ON runs(job,at DESC);
        CREATE TABLE IF NOT EXISTS imports (source TEXT PRIMARY KEY);
    """)
    return db


def next_due(schedule, after):
    kind = schedule.get("kind")
    if kind == "at":
        instant = dt.datetime.fromisoformat(schedule["at"].replace("Z", "+00:00"))
        if instant.tzinfo is None:
            raise ValueError("A one-time schedule needs a timezone")
        stamp = int(instant.timestamp() * 1000)
        return stamp if stamp > after else None
    if kind == "every":
        interval = schedule["everyMs"]
        if type(interval) is not int or not 0 < interval <= 8640000000000000:
            raise ValueError("Invalid interval")
        anchor = schedule.setdefault("anchorMs", after)
        return anchor + max(0, (after - anchor) // interval + 1) * interval
    if kind == "cron":
        expr = schedule["expr"]
        if not isinstance(expr, str) or len(expr) > 256 or len(expr.split()) not in (5, 6):
            raise ValueError("Expected a five- or six-field cron expression")
        zone = ZoneInfo(schedule["tz"]) if schedule.get("tz") else dt.datetime.now().astimezone().tzinfo
        base = dt.datetime.fromtimestamp(after / 1000, zone)
        return int(croniter(expr, base, second_at_beginning=True).get_next(dt.datetime).timestamp() * 1000)
    raise ValueError("Unsupported schedule")


def owner(job):
    return (job.get("owner") or {}).get("sessionKey") or job.get("sessionKey")


def save_job(db, job):
    due = job["state"].get("nextRunAtMs") if job["enabled"] else None
    db.execute("INSERT OR REPLACE INTO jobs VALUES (?,?,?,?)",
               (job["id"], owner(job), json.dumps(job), due))


def get_job(db, key, scope=None):
    row = db.execute("SELECT body FROM jobs WHERE id=? AND (? IS NULL OR owner=?)",
                     (key, scope, scope)).fetchone()
    if not row:
        raise ValueError("Task unavailable in this conversation")
    return json.loads(row["body"])


def enqueue(db, job, when):
    if db.execute("SELECT 1 FROM runs WHERE job=? AND phase IN ('queued','running')", (job["id"],)).fetchone():
        raise ValueError("This task is queued or running")
    run_id = uid()
    run = {"runId": run_id, "jobId": job["id"], "jobName": job["name"], "runAtMs": when,
           "job": {"id":job["id"], "name":job["name"], "owner":job.get("owner")}, "status":"queued"}
    db.execute("INSERT INTO runs VALUES (?,?,?,?,?,?,?)",
               (run_id, job["id"], owner(job), when, "queued", json.dumps(run), "[]"))
    return run_id


def validate(job):
    if not isinstance(job["name"], str) or not job["name"].strip() or len(job["name"]) > 256:
        raise ValueError("Give the task a name of up to 256 characters")
    payload = job["payload"]
    if payload.get("kind") != "agentTurn" or not isinstance(payload.get("message"), str) or not 0 < len(payload["message"].strip()) <= 65536:
        raise ValueError("A scheduled task needs an agent prompt of up to 65536 characters")
    if job.get("sessionTarget", "isolated") != "isolated" or (job.get("delivery") or {}).get("mode", "none") != "none":
        raise ValueError("Use an isolated task with delivery mode none")
    if type(job["enabled"]) is not bool:
        raise ValueError("enabled must be a boolean")


def rpc(db, request):
    action, p, scope = request["action"], request["params"], request.get("scope")
    if scope is not None:
        uuid.UUID(scope)
    if not isinstance(p, dict):
        raise ValueError("Expected an object")
    limit = max(1, min(100, int(p.get("limit", 100))))
    offset = max(0, int(p.get("offset", 0)))
    key = p.get("id")
    stamp = now()
    if action == "status":
        return {"enabled":True, "jobs":db.execute("SELECT count(*) FROM jobs WHERE ? IS NULL OR owner=?", (scope,scope)).fetchone()[0]}
    if action == "list":
        sql = "SELECT body FROM jobs WHERE (? IS NULL OR owner=?) AND (? OR json_extract(body,'$.enabled')) ORDER BY json_extract(body,'$.createdAtMs') DESC,id LIMIT ? OFFSET ?"
        rows = db.execute(sql, (scope,scope,bool(p.get("includeDisabled")),limit+1,offset)).fetchall()
        return {"jobs":[json.loads(r[0]) for r in rows[:limit]], "hasMore":len(rows)>limit}
    if action == "runs":
        rows = db.execute("""SELECT body FROM runs WHERE phase='finished' AND (? IS NULL OR job=?)
            AND (? IS NULL OR owner=?) ORDER BY at DESC,id LIMIT ? OFFSET ?""",
            (key,key,scope,scope,limit+1,offset)).fetchall()
        return {"entries":[json.loads(r[0]) for r in rows[:limit]], "hasMore":len(rows)>limit}
    if action == "history":
        row = db.execute("""SELECT messages FROM runs WHERE job=? AND (? IS NULL OR owner=?)
             AND (id=? OR (? IS NULL AND at=?))""", (key,scope,scope,p.get("runId"),p.get("runId"),p.get("runAtMs"))).fetchone()
        if not row:
            raise ValueError("Run history unavailable")
        return {"messages":json.loads(row[0])[-limit:]}
    db.execute("BEGIN IMMEDIATE")
    if action == "add":
        session = scope or p.get("sessionKey")
        if session is not None:
            try:
                uuid.UUID(session)
            except ValueError as e:
                raise ValueError("Send the first chat message before creating a task for this chat") from e
        job = {k:p[k] for k in ("name","schedule","payload","delivery","sessionTarget","deleteAfterRun") if k in p}
        job.update(id=uid(),createdAtMs=stamp,enabled=p.get("enabled",True),configRevision=uid(),state={})
        if session:
            job["owner"] = {"sessionKey":session}
        validate(job)
        due = next_due(job["schedule"], stamp)
        if due is None and job["enabled"]:
            raise ValueError("Choose a future date")
        job["state"]["nextRunAtMs"] = due if job["enabled"] else None
        save_job(db,job)
        return job
    job = get_job(db, key, scope)
    if action == "get":
        return job
    if p.get("expectedConfigRevision") not in (None, job["configRevision"]):
        raise ValueError("Task changed. Refresh before saving")
    if action == "remove":
        db.execute("DELETE FROM runs WHERE job=? AND phase='queued'", (key,))
        db.execute("DELETE FROM jobs WHERE id=?", (key,))
        return {"removed":True}
    if action == "run":
        if job.get("migrationError"):
            raise ValueError(job["migrationError"])
        return {"enqueued":True, "runId":enqueue(db,job,stamp)}
    if action == "update":
        patch = p["patch"]
        for field in patch:
            if field not in {"name","enabled","schedule","payload"}:
                raise ValueError("Unsupported task update")
        for field in ("name","enabled","schedule"):
            if field in patch:
                job[field] = patch[field]
        if "payload" in patch:
            job["payload"].update(patch["payload"])
        if job.get("migrationError") and job["enabled"]:
            raise ValueError(job["migrationError"])
        validate(job)
        if "schedule" in patch or "enabled" in patch:
            due = next_due(job["schedule"],stamp)
            if due is None and job["enabled"]:
                raise ValueError("Choose a future date")
            job["state"]["nextRunAtMs"] = due if job["enabled"] else None
        job["configRevision"] = uid()
        save_job(db,job)
        return job
    raise ValueError("Unknown scheduler method")


class NativeTurn:
    def __init__(self):
        from websocket import create_connection
        stream = socket.socket(socket.AF_UNIX)
        stream.settimeout(10)
        stream.connect(SOCKET)
        self.connection = create_connection("ws://localhost/", socket=stream, timeout=15)
        self.serial = 0
        self.deadline = time.monotonic() + 900
        self.thread = None
        self.result = None
        self.messages = []
        self.completed = {}

    def send(self, value):
        self.connection.send(json.dumps(value))

    def receive(self):
        from websocket import WebSocketTimeoutException
        if time.monotonic() > self.deadline:
            raise ValueError("Scheduled turn exceeded its 15-minute limit")
        try:
            msg = json.loads(self.connection.recv())
        except WebSocketTimeoutException:
            return {}
        if "method" in msg and "id" in msg:
            # An unattended run has no human available to grant new permissions.
            self.send({"id":msg["id"], "error":{"code":-32000,"message":"Scheduled run cannot grant interactive approval"}})
        elif self.thread and msg.get("params",{}).get("threadId") == self.thread:
            params = msg["params"]
            if msg.get("method") == "item/completed" and params.get("item",{}).get("type") == "agentMessage":
                item = params["item"]
                self.completed[item["id"]] = item.get("text","")[:65536]
            if msg.get("method") == "turn/completed":
                self.result = params["turn"]
        return msg

    def call(self, method, params):
        self.serial += 1
        identity = self.serial
        self.send({"id":identity,"method":method,"params":params})
        while True:
            reply = self.receive()
            if reply.get("id") == identity and "method" not in reply:
                if "error" in reply:
                    raise ValueError(reply["error"].get("message", "Native Codex request failed"))
                return reply["result"]

    def run(self, job, run_id):
        self.call("initialize", {"clientInfo":{"name":"d4c-scheduler","version":"1"},"capabilities":{"experimentalApi":True}})
        self.send({"method":"initialized"})
        payload = job["payload"]
        options = {"ephemeral":True,"approvalPolicy":"on-request","sandbox":"read-only"}
        if payload.get("toolsAllow") == []:
            options["config"] = {"features.shell_tool":False,"features.apps":False,
                                 "features.plugins":False,"features.computer_use":False,
                                 "web_search":"disabled"}
        if payload.get("model"):
            options["model"] = payload["model"].removeprefix("openai/")
        thread = self.call("thread/start",options)["thread"]
        self.thread = thread["id"]
        prompt = payload["message"]
        args = {"threadId":self.thread,"input":[{"type":"text","text":prompt}],"clientUserMessageId":run_id}
        if payload.get("thinking"):
            args["effort"] = {"off":"none"}.get(payload["thinking"],payload["thinking"])
        self.call("turn/start",args)
        while self.result is None:
            self.receive()
        self.messages = [{"role":"user","content":prompt}] + [
            {"role":"assistant","content":text} for text in self.completed.values()]
        if self.result.get("status") != "completed":
            raise ValueError((self.result.get("error") or {}).get("message") or self.result["status"])
        return "\n\n".join(self.completed.values())[:65536]


def finish(db, run, messages, error=None):
    stamp = now()
    run.update(action="finished",status="error" if error else "ok",ts=stamp,durationMs=stamp-run["runAtMs"])
    if error:
        run["error"] = str(error)[:2048]
    db.execute("UPDATE runs SET phase='finished',body=?,messages=? WHERE id=?",
               (json.dumps(run),json.dumps(messages),run["runId"]))
    row = db.execute("SELECT body FROM jobs WHERE id=?", (run["jobId"],)).fetchone()
    if row:
        job = json.loads(row[0])
        state = job["state"]
        state.pop("runningAtMs",None)
        state.update(lastRunAtMs=run["runAtMs"],lastRunStatus=run["status"],lastDurationMs=run["durationMs"])
        if error:
            state["lastError"] = run["error"]
        else:
            state.pop("lastError",None)
        if job.get("deleteAfterRun") and job["schedule"]["kind"] == "at" and not error and not job["enabled"]:
            db.execute("DELETE FROM jobs WHERE id=?", (job["id"],))
        else:
            save_job(db,job)


def tick(db):
    # systemd runs one worker at a time. A persisted running receipt means a prior
    # process exited before committing its result. Never repeat its side effects.
    import fcntl
    with (ROOT / "worker.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        with db:
            db.execute("BEGIN IMMEDIATE")
            for row in db.execute("SELECT body FROM runs WHERE phase='running'").fetchall():
                finish(db,json.loads(row[0]),[],"Scheduler stopped before recording completion")
            stamp = now()
            for row in db.execute("SELECT body FROM jobs WHERE due<=? ORDER BY due LIMIT 100",(stamp,)).fetchall():
                job = json.loads(row["body"])
                try:
                    enqueue(db,job,stamp)
                except ValueError:
                    pass  # An earlier invocation is still queued.
                due = next_due(job["schedule"],stamp)
                job["state"]["nextRunAtMs"] = due
                if due is None:
                    job["enabled"] = False
                save_job(db,job)
            row = db.execute("SELECT body FROM runs WHERE phase='queued' ORDER BY at,id LIMIT 1").fetchone()
            if not row:
                return
            run = json.loads(row[0])
            job = get_job(db,run["jobId"])
            db.execute("UPDATE runs SET phase='running' WHERE id=?",(run["runId"],))
            job["state"]["runningAtMs"] = stamp
            save_job(db,job)
        client, error, messages = None, None, []
        try:
            client = NativeTurn()
            run["summary"] = client.run(job,run["runId"])
        except Exception as exc:
            error = str(exc) or type(exc).__name__
        finally:
            if client:
                messages = client.messages
                try:
                    client.connection.close(timeout=2)
                except OSError:
                    pass
        with db:
            db.execute("BEGIN IMMEDIATE")
            finish(db,run,messages,error)


def archived_messages(archive, manifest, task):
    session = manifest["sessions"].get(task["child_session_key"], {})
    thread = manifest["threads"].get(session.get("threadId"), {})
    if not thread.get("path"):
        return []
    root = (archive / "native").resolve()
    path = (root / thread["path"]).resolve()
    if not path.is_relative_to(root) or not path.is_file():
        return []
    messages = []
    with path.open() as stream:
        for line in stream:
            entry = json.loads(line)
            payload = entry.get("payload",{})
            if entry.get("type") != "response_item" or payload.get("type") != "message" or payload.get("role") not in ("user","assistant"):
                continue
            text = "\n".join(p["text"] for p in payload.get("content",[]) if isinstance(p.get("text"),str))
            if text:
                messages.append({"role":payload["role"],"content":text[:65536]})
    return messages[-100:]


def enrich_receipts(db, legacy, archive, manifest):
    marker = str(archive.resolve()) + "#history-v1"
    if db.execute("SELECT 1 FROM imports WHERE source=?",(marker,)).fetchone():
        return 0
    joined = legacy.execute("""SELECT t.*, r.receipt_id, r.request_run_id FROM task_runs t
        JOIN cron_run_receipts r ON t.source_id=r.job_id AND t.started_at=r.started_at_ms
        WHERE t.task_kind='automation_run'""")
    count = 0
    for task in joined:
        old_id = task["request_run_id"] or task["receipt_id"]
        receipt = db.execute("SELECT body FROM runs WHERE id=? AND job=?",(old_id,task["source_id"])).fetchone()
        if not receipt:
            continue
        run = json.loads(receipt[0])
        details = json.loads(task["detail_json"] or "{}")
        native_id = details.get("runId") or old_id
        summary = details.get("summary") or task["terminal_summary"]
        messages = archived_messages(archive,manifest,task)
        if summary:
            run["summary"] = summary
            if not messages:
                messages = [{"role":"assistant","content":summary}]
        run["runId"] = native_id
        db.execute("UPDATE runs SET id=?,body=?,messages=? WHERE id=?",
                   (native_id,json.dumps(run),json.dumps(messages),old_id))
        count += 1
    db.execute("INSERT INTO imports VALUES(?)",(marker,))
    return count


def import_archive(db, archive):
    source = str(archive.resolve())
    with db:
        db.execute("BEGIN IMMEDIATE")
        legacy = sqlite3.connect((archive/"databases/openclaw.sqlite").resolve().as_uri()+"?mode=ro",uri=True)
        legacy.row_factory = sqlite3.Row
        manifest = json.loads((archive/"manifest.json").read_text())
        mapping = manifest["sessions"]
        if db.execute("SELECT 1 FROM imports WHERE source=?",(source,)).fetchone():
            return {"alreadyImported":True, "histories":enrich_receipts(db,legacy,archive,manifest)}
        archived_jobs = {}
        receipts = 0
        for row in legacy.execute("SELECT job_json,state_json FROM cron_jobs"):
            job = json.loads(row["job_json"])
            job["state"] = json.loads(row["state_json"] or "{}")
            job["state"].pop("runningAtMs",None)
            original_owner = owner(job)
            if original_owner:
                target = mapping.get(original_owner,{}).get("threadId")
                if not target:
                    job["migrationError"] = "The originating chat was not mapped to a native thread"
                else:
                    job["owner"] = {"sessionKey":target}
                    job["sessionKey"] = target
            if job.get("declarationKey") == "memory-core:memory-dreaming-promotion":
                job["migrationError"] = "This maintenance trigger belongs to the retired OpenClaw memory plugin"
            elif job["payload"].get("toolsAllow") not in (None, [], ["*"]):
                job["migrationError"] = "This legacy tool allowlist needs a native permission profile"
            elif job.get("sessionTarget") != "isolated" or job.get("delivery",{}).get("mode","none") != "none":
                job["migrationError"] = "The legacy target or delivery connector requires migration"
            if job.get("migrationError"):
                job["enabled"] = False
                job["state"]["lastError"] = job["migrationError"]
            elif job.get("enabled") and (job["state"].get("nextRunAtMs") or 0) <= now():
                due = next_due(job["schedule"],now())
                job["state"]["nextRunAtMs"] = due
                job["enabled"] = due is not None
            job["configRevision"] = uid()
            if db.execute("SELECT 1 FROM jobs WHERE id=?",(job["id"],)).fetchone():
                raise ValueError("Existing task conflicts with an archived task")
            save_job(db,job)
            archived_jobs[job["id"]] = job
        for row in legacy.execute("SELECT * FROM cron_run_receipts ORDER BY started_at_ms"):
            job = archived_jobs.get(row["job_id"],{})
            stamp = row["started_at_ms"] or row["finished_at_ms"] or 0
            runid = row["request_run_id"] or row["receipt_id"]
            run = {"runId":runid,"jobId":row["job_id"],"jobName":job.get("name","Archived task"),
                   "job":{"id":row["job_id"],"name":job.get("name"),"owner":job.get("owner")},
                   "action":"finished","runAtMs":stamp,"ts":row["finished_at_ms"] or stamp,
                   "status":row["status"],"error":row["error_text"],"source":"legacy-receipt"}
            db.execute("INSERT INTO runs VALUES(?,?,?,?,?,?,?)",
                       (runid,row["job_id"],owner(job),stamp,"finished",json.dumps(run),"[]"))
            receipts += 1
        db.execute("INSERT INTO imports VALUES(?)",(source,))
        return {"jobs":len(archived_jobs),"receipts":receipts,"histories":enrich_receipts(db,legacy,archive,manifest),"unavailable":sum(bool(j.get("migrationError")) for j in archived_jobs.values())}


def main():
    global ROOT
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory",type=Path,default=ROOT)
    parser.add_argument("command",choices=("rpc","tick","import"))
    parser.add_argument("archive",nargs="?",type=Path)
    args = parser.parse_args()
    ROOT = args.directory
    os.umask(0o077)
    db = open_store(ROOT)
    if args.command == "rpc":
        try:
            raw = sys.stdin.buffer.read(1024*1024+1)
            if len(raw) > 1024*1024:
                raise ValueError("Task request exceeds 1 MB")
            with db:
                result = rpc(db,json.loads(raw))
            print(json.dumps({"result":result}))
        except (KeyError, ValueError, TypeError, OverflowError, sqlite3.Error) as exc:
            print(json.dumps({"error":str(exc)}))
    elif args.command == "tick":
        tick(db)
    else:
        print(json.dumps(import_archive(db,args.archive)))


if __name__ == "__main__":
    main()
