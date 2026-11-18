#!/usr/bin/env python3
"""Migrate existing D4C schedules through the running Gateway without a restart."""
import json
import re
import subprocess


def call(method, params):
    result = subprocess.run(
        ['openclaw', 'gateway', 'call', method, '--json', '--params', json.dumps(params)],
        check=True, capture_output=True, text=True,
    )
    return json.loads(result.stdout)


def main():
    offset = 0
    while True:
        page = call('cron.list', {'includeDisabled': True, 'limit': 100, 'offset': offset})
        for job in page.get('jobs', []):
            owner = job.get('owner') or {}
            session = owner.get('sessionKey') or job.get('sessionKey') or ''
            if not re.fullmatch(r'agent:[^:]+:d4c-[^:]+', session):
                continue
            if job.get('sessionTarget') == 'isolated' and (job.get('delivery') or {}).get('mode') == 'none':
                continue
            patch = {'sessionTarget': 'isolated', 'delivery': {'mode': 'none'}}
            payload = job.get('payload') or {}
            if payload.get('kind') == 'systemEvent':
                patch['payload'] = {'kind': 'agentTurn', 'message': payload['text']}
            params = {'id': job['id'], 'patch': patch}
            if job.get('configRevision'):
                params['expectedConfigRevision'] = job['configRevision']
            call('cron.update', params)
            updated = call('cron.get', {'id': job['id']})
            if updated.get('sessionTarget') != 'isolated' or (updated.get('delivery') or {}).get('mode') != 'none':
                raise RuntimeError(f"Schedule isolation was not applied: {job['id']}")
            print(f"Isolated {job['id']}: {job.get('name', 'Scheduled task')}")
        if not page.get('hasMore'):
            break
        offset += len(page.get('jobs', []))


if __name__ == '__main__':
    main()
