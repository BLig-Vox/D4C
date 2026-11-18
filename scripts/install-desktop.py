#!/usr/bin/env python3
"""Install a built D4C binary and matching per-user Linux launcher/icons."""
from pathlib import Path
import os
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
source = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / 'target/debug/vox-d4c'
if not source.is_file():
    raise SystemExit(f'Build D4C first; missing {source}')
home = Path.home()
data = Path(os.environ.get('XDG_DATA_HOME', home / '.local/share'))
binary = home / '.local/bin/vox-d4c'
binary.parent.mkdir(parents=True, exist_ok=True)
staged = binary.with_suffix('.next')
shutil.copy2(source, staged)
staged.chmod(0o755)
staged.replace(binary)
for icon in (root / 'assets/icons').glob('*/site.blig.D4C.png'):
    destination = data / 'icons/hicolor' / icon.parent.name / 'apps' / icon.name
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(icon, destination)
entry = data / 'applications/site.blig.D4C.desktop'
entry.parent.mkdir(parents=True, exist_ok=True)
# Desktop-entry Exec arguments use double quoting, not shell syntax.
quoted = str(binary).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
entry.write_text((root / 'packaging/site.blig.D4C.desktop').read_text().replace('Exec=vox-d4c', f'Exec="{quoted}"'))
for command in (['update-desktop-database', str(entry.parent)], ['gtk-update-icon-cache', '-f', '-t', str(data / 'icons/hicolor')]):
    if shutil.which(command[0]):
        subprocess.run(command, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(f'Installed {binary}\nLauncher: {entry}')
