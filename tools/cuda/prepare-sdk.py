"""Fetch checksum-pinned NVIDIA headers and optional ptxas into a local cache."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import urllib.request
import zipfile

WORKSPACE = Path(__file__).resolve().parents[2]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def package(spec, root, offline):
    path = root / spec['file']
    if not path.exists():
        if offline:
            raise FileNotFoundError(f'Offline CUDA SDK cache is missing {path}')
        temporary = path.with_suffix('.partial')
        with urllib.request.urlopen(spec['url'], timeout=60) as response, temporary.open('wb') as out:
            shutil.copyfileobj(response, out, 1024 * 1024)
        if digest(temporary) != spec['sha256']:
            raise ValueError(f'CUDA package checksum mismatch: {temporary}')
        temporary.replace(path)
    if digest(path) != spec['sha256']:
        raise ValueError(f'CUDA cache checksum mismatch: {path}')
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=WORKSPACE / 'artifacts/cuda-sdk')
    parser.add_argument('--compiler', action='store_true', help='Also fetch ptxas for cubin verification')
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    root = args.root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    lock = json.loads(Path(__file__).with_name('sdk-lock.json').read_text(encoding='utf-8'))
    with zipfile.ZipFile(package(lock['header_package'], root, args.offline)) as archive:
        for name, expected in lock['headers'].items():
            data = archive.read('nvidia/cuda_runtime/include/' + name)
            if hashlib.sha256(data).hexdigest() != expected:
                raise ValueError(f'CUDA header checksum mismatch: {name}')
            (root / name).write_bytes(data)
    if args.compiler:
        with zipfile.ZipFile(package(lock['compiler_package'], root, args.offline)) as archive:
            (root / 'ptxas.exe').write_bytes(archive.read('nvidia/cuda_nvcc/bin/ptxas.exe'))
    (root / 'provenance.json').write_text(json.dumps(lock, indent=2) + '\n', encoding='utf-8')
    print(f'Prepared checksum-verified NVIDIA SDK files in {root}')


if __name__ == '__main__':
    main()
