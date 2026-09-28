"""Lock Debian's standard installation using a real APT solver and signed indexes.

The selected root must already have successfully run apt-get update. Resolution
uses an empty, temporary status file, never the root's installed package state
or the pins that hide packages already bundled in kinakaze-base.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.parse

ROOT = Path(__file__).resolve().parents[1]
PRIORITIES = {'required', 'important', 'standard'}


def paragraphs(text):
    for paragraph in text.strip().split('\n\n'):
        fields, key = {}, None
        for line in paragraph.splitlines():
            if line.startswith((' ', '\t')) and key:
                fields[key] += '\n' + line
            elif ': ' in line:
                key, value = line.split(': ', 1)
                fields[key] = value
        if 'Package' in fields:
            yield fields


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'config/debian-standard.lock.json')
    parser.add_argument('--extra', action='append', default=['busybox', 'curl', 'zip', 'unzip', 'tree', 'openssh-server'])
    args = parser.parse_args()
    root, dist = args.root.resolve(), args.dist.resolve()
    command = [str(dist / 'worker.exe'), '--root', str(root), '--dist', str(dist), '--']

    def run(*argv):
        return subprocess.check_output(command + list(argv), timeout=180).decode('utf-8')

    records = list(paragraphs(run('/usr/bin/apt-cache', 'dumpavail')))
    by_file = {record['Filename']: record for record in records if 'Filename' in record}
    seeds = sorted({r['Package'] for r in records if r.get('Priority') in PRIORITIES or r.get('Essential') == 'yes'})
    with tempfile.NamedTemporaryFile(prefix='kinakaze-apt-status-', dir=root / 'tmp', delete=False) as status:
        status_path = Path(status.name)
    try:
        plan = run('/usr/bin/apt-get', '-o', 'Dir::State::status=/tmp/' + status_path.name,
                   '-o', 'Dir::Etc::PreferencesParts=/dev/null', '--print-uris', '--yes', '--download-only', '--install-recommends',
                   'install', *sorted(set(seeds) | set(args.extra)))
    finally:
        status_path.unlink()
    packages = []
    for uri, size in re.findall(r"^'(https://[^']+)'\s+\S+\s+(\d+)", plan, re.M):
        uri = urllib.parse.unquote(uri)
        candidates = [r for filename, r in by_file.items() if uri.endswith('/' + filename)]
        if len(candidates) != 1:
            raise ValueError('APT archive has no unique signed metadata record: ' + uri)
        r = candidates[0]
        source, separator, version = r.get('Source', r['Package']).partition(' (')
        if int(size) != int(r['Size']) or not re.fullmatch(r'[0-9a-f]{64}', r['SHA256']):
            raise ValueError('APT archive metadata has an invalid size or digest: ' + uri)
        packages.append(dict(package=r['Package'], version=r['Version'], architecture=r['Architecture'],
                             priority=r.get('Priority', 'optional'), source_package=source,
                             source_version=version.rstrip(')') if separator else r['Version'],
                             source_directory=r['Filename'].rsplit('/', 1)[0],
                             url=uri, filename=r['Filename'].rsplit('/', 1)[1],
                             size=int(size), sha256=r['SHA256']))
    names = {p['package'] for p in packages}
    if not set(seeds).issubset(names) or len(names) != len(packages):
        raise ValueError('APT plan does not contain every standard package exactly once')
    releases = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in sorted((root / 'var/lib/apt/lists').glob('*_InRelease'))}
    if not releases:
        raise ValueError('no signed repository release indexes found')
    lock = dict(schema=1, distribution='Debian bookworm', architecture='amd64',
                selection=dict(priorities=sorted(PRIORITIES), essential=True, recommends=True,
                               packages=seeds, extra_packages=sorted(set(args.extra))),
                signed_release_sha256=releases, packages=sorted(packages, key=lambda p: p['package']))
    args.output.write_text(json.dumps(lock, indent=2) + '\n', encoding='utf-8', newline='\n')
    print(f'Locked {len(seeds)} standard packages and their complete {len(packages)}-package APT plan')


if __name__ == '__main__':
    main()
