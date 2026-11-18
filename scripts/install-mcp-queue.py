#!/usr/bin/env python3
"""Install the durable MCP queue as the OpenClaw Gateway's own Unix user."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--state-dir', type=Path, default=Path(os.environ.get('OPENCLAW_STATE_DIR', Path.home() / '.openclaw')))
p.add_argument('--workspace', type=Path, help='Existing agent workspace; defaults to STATE_DIR/workspace')
p.add_argument('--openclaw-package-root', type=Path, help='Directory containing OpenClaw package.json')
p.add_argument('--gateway-unit', default='openclaw-gateway.service')
args = p.parse_args()
if not re.fullmatch(r'[A-Za-z0-9_.@-]+\.service', args.gateway_unit):
    p.error('Invalid user Gateway unit name')
root = Path(__file__).resolve().parents[1]
state = args.state_dir.expanduser().resolve()
workspace = (args.workspace or state / 'workspace').expanduser().resolve()
if not workspace.is_dir():
    p.error('Agent workspace does not exist; run OpenClaw onboarding first or pass --workspace')
binary = shutil.which('openclaw')
if not binary:
    p.error('openclaw is missing from PATH')
package = args.openclaw_package_root
if not package:
    candidate = Path(binary).resolve().parent
    package = candidate if (candidate / 'package.json').exists() else Path(subprocess.check_output(['npm', 'root', '-g'], text=True).strip()) / 'openclaw'
package = package.expanduser().resolve()
if not (package / 'package.json').is_file():
    p.error('OpenClaw package not found; pass --openclaw-package-root')
manifest = json.loads((package / 'package.json').read_text())
if manifest.get('name') != 'openclaw':
    p.error('Selected package is not OpenClaw')
target = workspace / 'skills/connect-mcp'
source = root / 'server/openclaw-skills/connect-mcp'
target.mkdir(parents=True, exist_ok=True)
for file in sorted(source.rglob('*'), key=lambda f: f.name == 'd4c-mcp.py'):
    if not file.is_file() or '__pycache__' in file.parts:
        continue
    out = target / file.relative_to(source)
    out.parent.mkdir(parents=True, exist_ok=True)
    data = file.read_bytes()
    if file.suffix == '.md':
        data = data.decode().replace('~/.openclaw/workspace', str(workspace)).encode()
    temp = out.with_suffix(out.suffix + '.new')
    temp.write_bytes(data)
    temp.chmod(0o600)
    temp.replace(out)
agents = workspace / 'AGENTS.md'
text = agents.read_text() if agents.exists() else ''
text = re.sub(r'<!-- d4c-live-mcp:start -->.*?<!-- d4c-live-mcp:end -->', '', text, flags=re.S)
agents.write_text(text.rstrip() + '\n\n' + (target / 'workspace-guidance.md').read_text())
def quote(value):
    value = str(value)
    if '\n' in value or '\r' in value:
        p.error('Newlines are not supported in unit paths')
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace('$', '$$') + '"'
units = Path.home() / '.config/systemd/user'
units.mkdir(parents=True, exist_ok=True)
unit = (root / 'server/systemd/d4c-mcp-queue.service').read_text()
unit = unit.replace('After=openclaw-gateway.service', 'After=' + args.gateway_unit)
unit = unit.replace('ExecStart=/usr/bin/python3 %h/.openclaw/workspace/skills/connect-mcp/scripts/d4c-mcp.py work', 'ExecStart=' + quote(sys.executable) + ' ' + quote(target / 'scripts/d4c-mcp.py') + ' work')
environment = {'OPENCLAW_STATE_DIR': str(state), 'OPENCLAW_PACKAGE_ROOT': str(package), 'PATH': os.environ.get('PATH', os.defpath)}
if os.environ.get('OPENCLAW_CONFIG_PATH'):
    environment['OPENCLAW_CONFIG_PATH'] = os.environ['OPENCLAW_CONFIG_PATH']
unit = unit.replace('[Service]', '[Service]\n' + '\n'.join('Environment=' + quote(k + '=' + v) for k, v in environment.items()))
(units / 'd4c-mcp-queue.service').write_text(unit)
shutil.copy2(root / 'server/systemd/d4c-mcp-queue.timer', units)
subprocess.run(['systemctl', '--user', 'daemon-reload'], check=True)
subprocess.run(['systemctl', '--user', 'enable', '--now', 'd4c-mcp-queue.timer'], check=True)
print(f'Installed {target}\nMCP queue enabled; Gateway was not restarted.')
