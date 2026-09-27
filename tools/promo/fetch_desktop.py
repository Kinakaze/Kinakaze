"""Fill the verified Debian cache for the promo desktop; leave the lock unchanged."""
import concurrent.futures
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
lock = json.loads((ROOT / 'tools/guest-deps/dependencies.lock.json').read_text(encoding='utf-8'))
packages = {p['package']: p for p in lock['packages']}
selected = set()


def visit(name):
    if name in selected:
        return
    p = packages[name]
    selected.add(name)
    for dep in p.get('requires', []):
        visit(dep)


def download(name):
    p = packages[name]
    path = ROOT / 'artifacts/guest-deps' / p['filename']
    if path.exists() and path.stat().st_size == p['size'] and hashlib.sha256(path.read_bytes()).hexdigest() == p['sha256']:
        return name, 'cached'
    temp = path.with_name(path.name + '.promo-download')
    result = subprocess.run(['curl.exe', '--fail', '--location', '--retry', '3', '--retry-delay', '1',
                             '--connect-timeout', '15', '--max-time', '90', '--silent', '--show-error',
                             '--output', str(temp), p['url']], capture_output=True)
    if result.returncode:
        return name, result.stderr.decode(errors='replace')[-180:]
    data = temp.read_bytes()
    if len(data) != p['size'] or hashlib.sha256(data).hexdigest() != p['sha256']:
        return name, 'integrity mismatch'
    temp.replace(path)
    return name, 'downloaded'


if __name__ == '__main__':
    for name in ['gnome-shell', 'gnome-control-center', 'gnome-terminal']:
        visit(name)
    print('Packages:', len(selected), flush=True)
    failed = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        for name, status in pool.map(download, sorted(selected)):
            print(name, status, flush=True)
            if status not in ('cached', 'downloaded'):
                failed.append(name)
    print('Failed:', failed, flush=True)
    sys.exit(bool(failed))
