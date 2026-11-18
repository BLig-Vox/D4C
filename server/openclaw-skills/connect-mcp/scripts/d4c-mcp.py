#!/usr/bin/env python3
"""Publish MCP definitions through the running Gateway; never restart it."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import uuid
from urllib.parse import urlsplit


def gateway(method, params):
    result = subprocess.run(['openclaw', 'gateway', 'call', method, '--params', json.dumps(params), '--json'], capture_output=True, text=True, timeout=60)
    if result.returncode:
        # Never echo config responses or arbitrary remote diagnostics that may hold secrets.
        raise ValueError('Gateway rejected the request. Inspect its redacted diagnostics; do not restart or edit its state offline.')
    return json.loads(result.stdout)


def bearer_reference(source):
    source = Path(source)
    fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError('Bearer file must be a regular owner-only file belonging to the Gateway user.')
        token = stream.read(16385).strip()
    if not token or len(token) > 16384 or re.search(r'[\s\x00-\x1f\x7f]', token):
        raise ValueError('Bearer file must contain only the token, with no whitespace.')
    root = Path(os.environ.get('OPENCLAW_STATE_DIR', Path.home() / '.openclaw'))
    key = 'D4C_MCP_' + uuid.uuid4().hex.upper()
    env = root / '.env'
    with open(root / '.d4c-mcp-env.lock', 'a') as lock:
        os.chmod(lock.name, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        if env.is_symlink():
            raise ValueError('Refusing to replace a symlinked .env file.')
        previous = env.read_text() if env.exists() else ''
        # JSON quoting is accepted by dotenv and prevents newline/config injection.
        content = previous.rstrip('\n') + '\n' + key + '=' + json.dumps(token) + '\n'
        if len(content.encode()) > 900000:
            raise ValueError('Private credential store is full; no change applied')
        fd, name = tempfile.mkstemp(prefix='.d4c-mcp-env-', dir=root)
        try:
            with os.fdopen(fd, 'w') as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(name, env)
        finally:
            if os.path.exists(name):
                os.unlink(name)
    return 'Bearer ${' + key + '}'


def apply_request(args):
    snapshot = gateway('config.get', {})
    if snapshot.get('appliedConfigHash') != snapshot.get('configRevisionHash'):
        raise ValueError('Gateway has unapplied config. Resolve that before changing MCP.')
    config = snapshot.get('runtimeConfig', snapshot.get('config', {}))
    servers = config.get('mcp', {}).get('servers', {})
    if args.action == 'list':
        print(json.dumps({'servers': [{'name': k, 'enabled': v.get('enabled', True), 'transport': v.get('transport', 'stdio' if v.get('command') else 'streamable-http')} for k, v in servers.items()]}))
        return
    if config.get('gateway', {}).get('reload', {}).get('mode') == 'off':
        raise ValueError('Live config reload is disabled; ask the operator to enable it before configuring MCP.')
    if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}', args.name):
        raise ValueError('Use a short server name containing letters, numbers, underscores or hyphens.')
    if args.action == 'connect':
        url = urlsplit(args.url)
        if url.scheme != 'https' or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError('Use an HTTPS MCP URL without embedded credentials, query tokens or fragments.')
        definition = {'url': args.url, 'transport': args.transport, 'enabled': True, 'connectionTimeoutMs': 5000, 'requestTimeoutMs': 20000}
        if args.allow:
            definition['toolFilter'] = {'include': args.allow}
        if args.name in servers:
            # Updating only the same endpoint is safe; replacing an unrelated definition needs an explicit disconnect first.
            if servers[args.name].get('url') != args.url:
                raise ValueError('Name already belongs to another endpoint. Choose another name or explicitly disconnect it first.')
        if args.bearer_file:
            definition['headers'] = {key: None for key in servers.get(args.name, {}).get('headers', {}) if key.lower() == 'authorization' and key != 'Authorization'}
            definition['headers']['Authorization'] = bearer_reference(args.bearer_file)
        patch = {'mcp': {'servers': {args.name: definition}}}
    else:
        if args.name not in servers:
            print(json.dumps({'saved': False, 'name': args.name, 'status': 'already absent'}))
            return
        patch = {'mcp': {'servers': {args.name: None}}}
    params = {'raw': json.dumps(patch), 'baseHash': snapshot['hash'], 'response': 'summary', 'note': 'D4C live MCP connection'}
    if args.action == 'disconnect':
        # Gateway requires explicit acknowledgement for arrays removed with their containing server.
        def arrays(value, prefix):
            if isinstance(value, list):
                return [prefix]
            if isinstance(value, dict):
                return [item for key, child in value.items() for item in arrays(child, prefix + '.' + key)]
            return []
        params['replacePaths'] = arrays(servers[args.name], 'mcp.servers.' + args.name)
    elif args.allow:
        params['replacePaths'] = ['mcp.servers.' + args.name + '.toolFilter.include']
    result = gateway('config.patch', params)
    if result.get('restartPending') or result.get('restart', {}).get('scheduled'):
        raise ValueError('Unexpected restart requirement: inspect config application before proceeding.')
    applied = gateway('config.get', {})
    if applied.get('appliedConfigHash') != applied.get('configRevisionHash'):
        raise ValueError('MCP definition was saved but its runtime application is not confirmed.')
    live = applied.get('runtimeConfig', applied.get('config', {})).get('mcp', {}).get('servers', {})
    if args.action == 'disconnect' and args.name in live:
        raise ValueError('MCP removal is not active yet.')
    if args.action == 'connect' and (live.get(args.name, {}).get('url') != args.url or not live.get(args.name, {}).get('enabled', True)):
        raise ValueError('MCP connection configuration was superseded; inspect current definitions before retrying.')
    print(json.dumps({'saved': True, 'name': args.name, 'status': 'removed' if args.action == 'disconnect' else 'configured; verify live tool discovery on the next turn', 'changedPaths': result.get('changedPaths', [])}))


def main():
    from job_queue import enqueue, status, work
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest='action', required=True)
    subs.add_parser('list')
    query = subs.add_parser('status')
    query.add_argument('job', nargs='?')
    subs.add_parser('work', help=argparse.SUPPRESS)
    add = subs.add_parser('connect')
    add.add_argument('name')
    add.add_argument('url')
    add.add_argument('--transport', choices=['streamable-http','sse'], default='streamable-http')
    add.add_argument('--bearer-file')
    add.add_argument('--allow', action='append')
    remove = subs.add_parser('disconnect')
    remove.add_argument('name')
    args = parser.parse_args()
    if args.action == 'status':
        print(json.dumps(status(args.job)))
    elif args.action == 'work':
        work(gateway, apply_request)
    elif args.action == 'list':
        # Listing does not require a matching applied revision and never mutates state.
        snapshot = gateway('config.get', {})
        servers = snapshot.get('runtimeConfig',snapshot.get('config',{})).get('mcp',{}).get('servers',{})
        print(json.dumps({'applied':snapshot.get('appliedConfigHash') == snapshot.get('configRevisionHash'),
            'servers':[{'name':k,'enabled':v.get('enabled',True),'transport':v.get('transport','stdio' if v.get('command') else 'streamable-http')} for k,v in servers.items()]}))
    else:
        print(json.dumps(enqueue(args)))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
        print('MCP request failed; no success confirmed. Check queue status and redacted Gateway diagnostics.', file=sys.stderr)
        sys.exit(1)
