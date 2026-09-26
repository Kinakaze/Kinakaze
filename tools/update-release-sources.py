"""Lock corresponding Debian sources for exactly the prepared base image."""
import argparse
import hashlib
import json
import time
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'config/release-sources.lock.json')
    parser.add_argument('--cache', type=Path, default=ROOT / 'artifacts/guest-sources')
    args = parser.parse_args()
    records = json.loads((args.dist / 'rootfs/usr/share/kinakaze/bootstrap-packages.json').read_text(encoding='utf-8'))['packages']
    sources = {}
    args.cache.mkdir(parents=True, exist_ok=True)
    for record in records:
        key = record['source_package'], record['source_version']
        sources[key] = record
    output = []
    for (name, version), record in sorted(sources.items()):
        filename = f'{name}_{version.split(":")[-1]}.dsc'
        directory = record['url'].rsplit('/', 1)[0]
        cached = args.cache / filename
        if cached.is_file():
            data = cached.read_bytes()
        else:
            print(f'Fetching source metadata: {filename}', flush=True)
            for attempt in range(4):
                try:
                    with urllib.request.urlopen(f'{directory}/{filename}', timeout=45) as response:
                        data = response.read()
                    break
                except OSError:
                    if attempt == 3:
                        raise
                    time.sleep(attempt + 1)
            cached.write_bytes(data)
        files = [dict(filename=filename, url=f'{directory}/{filename}', size=len(data), sha256=hashlib.sha256(data).hexdigest())]
        in_checksums = False
        for line in data.decode().splitlines():
            if line == 'Checksums-Sha256:':
                in_checksums = True
            elif in_checksums:
                if not line.startswith(' '):
                    break
                digest, size, archive = line.split()
                if Path(archive).name != archive or '/' in archive or '\\' in archive:
                    raise ValueError(f'invalid source archive: {archive}')
                files.append(dict(filename=archive, url=f'{directory}/{archive}', size=int(size), sha256=digest))
        if len(files) < 2:
            raise ValueError(f'source descriptor has no SHA-256 archive checksums: {filename}')
        output.append(dict(package=name, version=version, files=files))
    args.output.write_text(json.dumps(dict(schema=1, packages=output), indent=2) + '\n', encoding='utf-8', newline='\n')
    print(f'Locked corresponding sources for {len(output)} packages')


if __name__ == '__main__':
    main()
