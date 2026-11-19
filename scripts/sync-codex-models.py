#!/usr/bin/env python3
"""Reconcile only Codex-routed model entries with a fresh account model catalog."""
import argparse
import copy
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import tempfile
import time


def reconcile(config, catalog, now):
    try:
        fetched=datetime.fromisoformat(catalog['fetched_at'].replace('Z','+00:00')).timestamp()
    except (KeyError,ValueError,TypeError) as error:
        raise ValueError('Account catalog has no valid fetch timestamp') from error
    if not -300 <= now-fetched <= 86400:
        raise ValueError('Refresh the account catalog before reconciling (maximum age: 24 hours)')
    valid=[m for m in catalog.get('models',[]) if isinstance(m,dict) and isinstance(m.get('slug'),str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,127}',m['slug'])]
    available=[m['slug'] for m in valid if m.get('visibility')=='list']
    if not available:
        raise ValueError('Account catalog has no visible models; configuration was not changed')
    result=copy.deepcopy(config)
    defaults=result.setdefault('agents',{}).setdefault('defaults',{})
    models=defaults.setdefault('models',{})
    allowed={'openai/'+m['slug'] for m in valid}
    removed={key for key,value in models.items() if key.startswith('openai/') and isinstance(value,dict) and isinstance(value.get('agentRuntime'),dict) and value['agentRuntime'].get('id')=='codex' and key not in allowed}
    policy=defaults.get('modelPolicy',{})
    if isinstance(policy,dict) and isinstance(policy.get('allow'),list):
        before_allow=policy['allow'];after_allow=[key for key in before_allow if key not in removed]
        if before_allow and not after_allow:
            raise ValueError('No explicit model-policy choices would remain; review the policy before applying')
        policy['allow']=after_allow
    for key in removed:del models[key]
    for slug in available:models.setdefault('openai/'+slug,{'agentRuntime':{'id':'codex'}})
    selected=defaults.get('model')
    def replacement():
        # Exact policy evaluation belongs to OpenClaw. A restricted policy needs
        # an operator-selected default rather than guessing wildcard semantics.
        if isinstance(policy,dict) and (policy.get('allow') or policy.get('deny')):
            raise ValueError('Default model was removed under a restricted policy; select a permitted replacement first')
        return 'openai/'+available[0]
    if isinstance(selected,str) and selected in removed:defaults['model']=replacement()
    elif isinstance(selected,dict):
        if selected.get('primary') in removed:selected['primary']=replacement()
        if isinstance(selected.get('fallbacks'),list):selected['fallbacks']=[key for key in selected['fallbacks'] if key not in removed]
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config',type=Path,required=True)
    parser.add_argument('--catalog',type=Path,required=True,help='models_cache.json from the actual authenticated Codex runtime home')
    parser.add_argument('--apply',action='store_true',help='Back up and atomically replace config; default is dry run')
    args=parser.parse_args()
    if args.config.is_symlink() or not args.config.is_file():parser.error('Config must be a regular file, not a symlink')
    before=args.config.read_bytes();config=json.loads(before)
    updated=reconcile(config,json.loads(args.catalog.read_bytes()),time.time())
    old=config.get('agents',{}).get('defaults',{}).get('models',{})
    new=updated['agents']['defaults']['models']
    print(json.dumps({'apply':args.apply,'removed':sorted(set(old)-set(new)),'added':sorted(set(new)-set(old))}))
    if updated==config or not args.apply:return
    stat=args.config.stat();backup=args.config.with_name(args.config.name+'.d4c-backup-'+datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ'))
    fd=os.open(backup,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(fd,'wb') as file:file.write(before);file.flush();os.fsync(file.fileno())
    temporary=None
    try:
        with tempfile.NamedTemporaryFile(dir=args.config.parent,prefix='.d4c-models-',delete=False) as file:
            temporary=Path(file.name);os.fchmod(file.fileno(),stat.st_mode & 0o777)
            os.fchown(file.fileno(),stat.st_uid,stat.st_gid)
            file.write((json.dumps(updated,indent=2)+'\n').encode());file.flush();os.fsync(file.fileno())
        if args.config.read_bytes()!=before:raise RuntimeError('Configuration changed concurrently; rerun reconciliation')
        os.replace(temporary,args.config)
        fd=os.open(args.config.parent,os.O_RDONLY|os.O_DIRECTORY)
        try:os.fsync(fd)
        finally:os.close(fd)
    finally:
        if temporary:temporary.unlink(missing_ok=True)
    print('Backup: '+str(backup))


if __name__=='__main__':main()
