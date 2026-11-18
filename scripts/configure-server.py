#!/usr/bin/env python3
"""Render deployment files for this server without modifying system services."""
import argparse
import json
from pathlib import Path
import pwd
import re

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--hostname', required=True, help='Certificate DNS name')
p.add_argument('--listen', required=True, help='Private interface address:port')
p.add_argument('--openclaw-user', required=True, help='Unix user running the Gateway')
p.add_argument('--output', type=Path, default=Path('target/deploy'))
args = p.parse_args()
observer = pwd.getpwnam(args.openclaw_user)
if not re.fullmatch(r'[A-Za-z0-9.-]+', args.hostname):
    p.error('Use a DNS hostname without a scheme or path')
if not re.fullmatch(r'[A-Za-z0-9.:[\]-]+', args.listen):
    p.error('Use a bind address:port')
try:
    account = pwd.getpwnam('d4c')
except KeyError:
    p.error('Create the d4c service account first (see docs/setup.md)')
root = Path(__file__).resolve().parents[1]
args.output.mkdir(parents=True, exist_ok=True)
for source in (root / 'server/deploy').iterdir():
    text = source.read_text().replace('@D4C_HOSTNAME@', args.hostname).replace('@D4C_LISTEN@', args.listen).replace('@D4C_UID@', str(account.pw_uid)).replace('/home/d4c', account.pw_dir).replace('@D4C_OBSERVER_UID@', str(observer.pw_uid))
    if source.suffix == '.json':
        json.loads(text)
    (args.output / source.name).write_text(text)
print(f'Rendered deployment files in {args.output}; review before installing.')
