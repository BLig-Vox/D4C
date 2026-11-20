#!/usr/bin/env python3
"""Package already-built Linux D4C client/server binaries and deployment files."""
import argparse
import gzip
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binaries', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--epoch', type=int, required=True)
    a = p.parse_args()
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    a.output.mkdir(parents=True, exist_ok=True)
    for server in (False, True):
        kind = 'd4c-server' if server else 'd4c'
        prefix = f'{kind}-{version}-linux-x86_64'
        names = ['LICENSE', 'README.md', 'INSTALL.md', 'docs/setup.md', 'docs/mcp.md', 'scripts/build-limited.py']
        if server:
            names += ['ATTRIBUTION.md','server/README.md','scripts/configure-server.py',
                      'scripts/stage-codex.py','scripts/activate-codex-home.py',
                      'vendor/ponytail/LICENSE','vendor/ponytail/skills/ponytail/SKILL.md',
                      'server/scripts/d4c-scheduler.py','server/scripts/d4c-refresh-codex.py']
            for directory in ['server/deploy','server/desktop']:
                names += [str(f.relative_to(ROOT)) for f in sorted((ROOT / directory).rglob('*'))
                          if f.is_file() and '__pycache__' not in f.parts and f.suffix != '.pyc']
        else:
            names += ['scripts/install-desktop.py', 'packaging/site.blig.D4C.desktop',
                      'vendor/winit/LICENSE', 'vendor/winit/D4C-PATCH.md',
                      'vendor/eframe/LICENSE-MIT', 'vendor/eframe/LICENSE-APACHE']
            names += [str(f.relative_to(ROOT)) for f in sorted((ROOT / 'assets/icons').rglob('*.png'))]
            names += [str(f.relative_to(ROOT)) for f in sorted((ROOT / 'assets/fonts').glob('*LICENSE*'))]
            names += [str(f.relative_to(ROOT)) for f in sorted((ROOT / 'assets/fonts').glob('*OFL*'))]
        files = [(name, ROOT / name, 0o644) for name in dict.fromkeys(names)]
        binary = 'vox-d4c-server' if server else 'vox-d4c'
        files.append(('bin/' + binary, a.binaries / binary, 0o755))
        for name, path, _ in files:
            if not path.is_file(): p.error('Missing payload: ' + str(path))
        output = a.output / (prefix + '.tar.gz')
        with tempfile.NamedTemporaryFile(dir=a.output, prefix='.d4c-package-') as temp:
            with gzip.GzipFile(fileobj=temp, mode='wb', filename='', mtime=0, compresslevel=6) as compressed:
                with tarfile.open(fileobj=compressed, mode='w|', format=tarfile.PAX_FORMAT) as archive:
                    for name, path, mode in sorted(files):
                        info = tarfile.TarInfo(prefix + '/' + name)
                        info.size, info.mode, info.mtime = path.stat().st_size, mode, a.epoch
                        with path.open('rb') as stream: archive.addfile(info, stream)
            temp.flush()
            os.fsync(temp.fileno())
            os.link(temp.name, output)
        print(json.dumps({'archive': str(output), 'files': len(files)}))


if __name__ == '__main__':
    main()
