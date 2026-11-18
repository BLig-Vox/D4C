"""Durable MCP jobs. Only the idle worker publishes configuration."""
from contextlib import redirect_stdout
import fcntl
import io
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
import time
from types import SimpleNamespace
import uuid
from urllib.parse import urlsplit

ROOT = Path(os.environ.get('OPENCLAW_STATE_DIR', Path.home() / '.openclaw'))
DIRECTORY = ROOT / 'd4c-mcp-jobs'
TERMINAL = {'applied','failed'}


def directory():
    DIRECTORY.mkdir(mode=0o700, parents=True, exist_ok=True)
    DIRECTORY.chmod(0o700)
    return DIRECTORY


def save(job):
    fd, temp = tempfile.mkstemp(prefix='.pending-', dir=directory())
    try:
        with os.fdopen(fd, 'w') as stream:
            json.dump(job, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, DIRECTORY / (job['id'] + '.json'))
        fd = os.open(DIRECTORY, os.O_RDONLY)
        try: os.fsync(fd)
        finally: os.close(fd)
    finally:
        if os.path.exists(temp): os.unlink(temp)


def jobs():
    result = []
    for file in directory().glob('*.json'):
        try:
            if file.stat().st_size <= 65536:
                job = json.loads(file.read_text())
                if re.fullmatch(r'[a-f0-9]{32}', job.get('id','')) and file.stem == job['id']:
                    result.append(job)
        except (OSError, ValueError):
            continue
    return sorted(result, key=lambda j:j['created'])


def public(job):
    return {k:job[k] for k in ('id','action','name','state','detail','created','finished','toolCount') if k in job}


def status(job_id=None):
    found = jobs()
    if job_id:
        found = [j for j in found if j['id'] == job_id]
        if not found: raise ValueError('No such MCP job')
    return {'jobs':[public(j) for j in found[-30:]]}


def enqueue(args):
    name = args.name
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,63}', name):
        raise ValueError('Invalid MCP name')
    if args.action == 'connect':
        url = urlsplit(args.url)
        if url.scheme != 'https' or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError('Use a credential-free HTTPS MCP URL')
    with open(directory() / '.queue.lock', 'a') as lock:
        os.chmod(lock.name,0o600)
        fcntl.flock(lock,fcntl.LOCK_EX)
        existing = jobs()
        if sum(j['state'] not in TERMINAL for j in existing) >= 32:
            raise ValueError('MCP queue is full')
        # Bound durable metadata without removing any pending request.
        for old in [j for j in existing if j['state'] in TERMINAL][:-128]:
            (DIRECTORY / (old['id'] + '.json')).unlink(missing_ok=True)
        job = {'id':uuid.uuid4().hex,'action':args.action,'name':name,'state':'queued',
               'detail':'Waiting for active chats to finish','created':time.time(), 'attempts':0}
        if args.action == 'connect':
            job.update(url=args.url, transport=args.transport, allow=args.allow)
            if args.bearer_file:
                fd = os.open(args.bearer_file,os.O_RDONLY | os.O_NOFOLLOW)
                with os.fdopen(fd) as stream:
                    meta = os.fstat(stream.fileno())
                    if not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
                        raise ValueError('Credential file must be private and owned by Gateway user')
                    token = stream.read(16385).strip()
                if not token or len(token)>16384 or re.search(r'[\s\x00-\x1f\x7f]',token):
                    raise ValueError('Invalid bearer file')
                file = DIRECTORY / (job['id'] + '.token')
                fd = os.open(file,os.O_WRONLY | os.O_CREAT | os.O_EXCL,0o600)
                with os.fdopen(fd,'w') as stream:
                    stream.write(token)
                    stream.flush(); os.fsync(stream.fileno())
                job['bearer_file'] = str(file)
        fingerprint_fields={k:job.get(k) for k in ('action','name','url','transport','allow')}
        if job.get('bearer_file'):
            fingerprint_fields['credentialHash']=hashlib.sha256(Path(job['bearer_file']).read_bytes()).hexdigest()
        job['requestFingerprint']=hashlib.sha256(json.dumps(fingerprint_fields,sort_keys=True).encode()).hexdigest()
        for previous in existing:
            if previous['state'] not in TERMINAL and previous.get('requestFingerprint')==job['requestFingerprint']:
                if job.get('bearer_file'): Path(job['bearer_file']).unlink(missing_ok=True)
                return {'queued':True,'jobId':previous['id'],'status':'Already queued; wait for this chat turn to finish'}
        save(job)
    return {'queued':True,'jobId':job['id'],'status':'Applies after active chat turns finish; inspect status before claiming connected'}


def idle(gateway):
    result = gateway('sessions.list',{})
    sessions = result.get('sessions')
    # Missing activity facts are not proof of idleness.
    return isinstance(sessions,list) and all(s.get('hasActiveRun') is False for s in sessions)


def work(gateway, apply_request):
    with open(directory() / '.worker.lock','a') as lock:
        os.chmod(lock.name,0o600)
        try: fcntl.flock(lock,fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError: return
        pending = [j for j in jobs() if j['state'] not in TERMINAL]
        if not pending: return
        # One writer per tick; later jobs cannot overtake an earlier mutation.
        job = pending[0]
        try:
            if not idle(gateway): return
            snapshot = gateway('config.get',{})
            if not snapshot.get('valid'): raise ValueError('Invalid Gateway configuration')
            if snapshot.get('appliedConfigHash') != snapshot.get('configRevisionHash'):
                if job.get('recoveryAttempted'):
                    raise ValueError('Config still unapplied after recovery')
                # This is outside the model turn and only runs when all chats are idle.
                # The supported lifecycle owner drains any racing admitted work.
                job['recoveryAttempted'] = True
                job['state']='recovering'; job['detail']='Recovering the idle Codex runtime'
                save(job)
                result=subprocess.run(['openclaw','plugins','reload','codex','--json'],capture_output=True,text=True,timeout=70)
                if result.returncode: raise ValueError('Plugin recovery refused')
                receipt=json.loads(result.stdout)
                if not receipt.get('ok') or receipt.get('restartRequired'):
                    raise ValueError('Plugin recovery not confirmed')
                snapshot=gateway('config.get',{})
                if snapshot.get('appliedConfigHash') != snapshot.get('configRevisionHash'):
                    raise ValueError('Unapplied configuration remains')
            if not idle(gateway): return
            if job['action']=='connect':
                # SDK discovery validates auth and transport before any config write.
                job['state']='checking';job['detail']='Checking remote MCP authentication and tools';save(job)
                result=subprocess.run(['node',str(Path(__file__).with_name('probe.mjs'))],
                    input=json.dumps(job),capture_output=True,text=True,timeout=25)
                proof=json.loads(result.stdout) if result.stdout.strip() else {}
                if result.returncode or not proof.get('ok'):
                    job['state']='failed';job['detail']=proof.get('reason','Connection check failed; previous settings kept')
                    job['finished']=time.time();save(job);return
                job['toolCount']=proof['toolCount']
                job['authUnchanged']=proof.get('authUnchanged',False)
            # A new chat may have started during the network check. Recheck before publish.
            if not idle(gateway):
                job['state']='queued';job['detail']='Checked; waiting for active chats to finish';save(job);return
            job['state']='applying';job['detail']='Publishing MCP configuration';save(job)
            args=SimpleNamespace(**{**{'bearer_file':None,'allow':None},**job})
            if job.get('authUnchanged'): args.bearer_file=None
            with redirect_stdout(io.StringIO()): apply_request(args)
            job['state']='applied';job['detail']='Connection verified and applied; available on the next message' if job['action']=='connect' else 'Connection removed'
            job['finished']=time.time();save(job)
        except (OSError,ValueError,subprocess.SubprocessError,KeyError):
            job['attempts'] = job.get('attempts',0)+1
            job['state']='failed' if job['attempts']>=3 else 'queued'
            job['detail']='Gateway update not confirmed; inspect redacted diagnostics' if job['state']=='failed' else 'Gateway busy/unavailable; retrying without restarting it'
            if job['state']=='failed': job['finished']=time.time()
            save(job)
        finally:
            if job.get('state') in TERMINAL and job.get('bearer_file'):
                Path(job['bearer_file']).unlink(missing_ok=True)
