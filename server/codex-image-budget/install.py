#!/usr/bin/env python3
"""Install the version-checked D4C screenshot projection in the local Codex relay."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--plugin-root', type=Path, required=True)
p.add_argument('--apply', action='store_true', help='Write after compatibility checks; activate in an idle Gateway restart')
a = p.parse_args()
root = a.plugin_root.resolve()
manifest = json.loads((root / 'package.json').read_text())
if manifest.get('name') != '@openclaw/codex' or manifest.get('version') != '2026.11.5':
    p.error('Unsupported Codex plugin version; no files changed')
setup = root / 'dist/.setup'
files = list(setup.glob('inference-dispatch-*.mjs'))
if len(files) != 1:
    p.error('Expected exactly one dispatch module')
dispatch = files[0]
proxies = list(setup.glob('inference-proxy-*.mjs'))
if len(proxies) != 1:
    p.error('Expected exactly one inference proxy module')
proxy = proxies[0]
marker = '// d4c-image-budget-v1'
old = dispatch.read_text()
source = Path(__file__).with_name('guard.mjs').read_text()
if marker in old:
    if (setup / 'd4c-image-budget.mjs').read_text() != source:
        p.error('Installed guard differs; review before replacing')
    receipt = json.loads((setup / 'd4c-image-budget-receipt.json').read_text())
    expected = {dispatch.name, proxy.name, 'd4c-image-budget.mjs'}
    if set(receipt.get('files', {})) != expected:
        p.error('Invalid installed receipt; review before replacing')
    for name, digest in receipt['files'].items():
        if hashlib.sha256((setup / name).read_bytes()).hexdigest() != digest:
            p.error('Installed module changed; review before replacing')
    print('D4C image budget already installed; module hashes verified')
    raise SystemExit(0)
needle = 'const rewritten = prepared && (prepared.body !== value || !isUtf8(bytes)) ? Buffer.from(JSON.stringify(prepared.body)) : bytes;'
replacements = {
    needle: 'const projected = projectScreenshotHistory(prepared?.body ?? value);\n\t\t\tconst rewritten = projected && (projected !== value || !isUtf8(bytes)) ? Buffer.from(JSON.stringify(projected)) : bytes;',
    'readProxyBody(req, MAX_BODY_BYTES,': 'readProxyBody(req, 67108864,',
    '{ maxOutputLength: MAX_BODY_BYTES }': '{ maxOutputLength: 67108864 }',
}
for before, after in replacements.items():
    if old.count(before) != 1:
        p.error('Unsupported dispatch layout; no files changed')
    old = old.replace(before, after)
if 'if (rewritten.length > 33554432)' not in old:
    p.error('Missing original outgoing 32MiB guard; no files changed')
updated = marker + '\nimport { projectScreenshotHistory } from "./d4c-image-budget.mjs";\n' + old
proxy_old = proxy.read_text()
# The first maxPayload belongs to the private inbound server. Keep the upstream
# response-size guard unchanged. Verify both known occurrences before touching it.
if proxy_old.count('maxPayload: MAX_BODY_BYTES,') != 2:
    p.error('Unsupported proxy layout; no files changed')
proxy_updated = marker + '\n' + proxy_old.replace('maxPayload: MAX_BODY_BYTES,', 'maxPayload: 67108864,', 1)
outputs = {dispatch: updated, proxy: proxy_updated, setup / 'd4c-image-budget.mjs': source}
with tempfile.TemporaryDirectory() as directory:
    for dest, data in outputs.items():
        temp = Path(directory) / dest.name
        temp.write_text(data)
        subprocess.run(['node', '--check', str(temp)], check=True)
print('Compatible plugin; outgoing limit stays32MiB, local ingestion bounded64MiB')
if not a.apply:
    print('No files changed; use --apply after checking active work')
    raise SystemExit(0)
for dest, data in outputs.items():
    if dest.exists():
        backup = dest.with_suffix(dest.suffix + '.d4c-original')
        if not backup.exists():
            shutil.copy2(dest, backup)
    temp = dest.with_suffix(dest.suffix + '.d4c-new')
    temp.write_text(data)
    temp.chmod(0o644)
    temp.replace(dest)
receipt = {'pluginVersion': manifest['version'], 'guard': 'v1', 'files': {f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in outputs}}
(setup / 'd4c-image-budget-receipt.json').write_text(json.dumps(receipt,indent=2))
print('Installed; original modules backed up. Activate with an idle Gateway restart.')
