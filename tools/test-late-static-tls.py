"""Compile and run a late initial-exec TLS regression in a Linux guest."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import uuid

from init_pool import InitPool


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--libgomp', help='Guest path to an actual libgomp to test with four threads')
    args = parser.parse_args()
    source = Path(__file__).resolve().parents[1] / 'tests/guest'
    guest = '/tmp/late-static-tls-' + uuid.uuid4().hex
    staging = args.root.resolve() / guest.lstrip('/')
    staging.mkdir(parents=True)
    subprocess.run(['clang', '--target=x86_64-linux-gnu', '-fuse-ld=lld',
                    '-fPIC', '-shared', '-nostdlib', '-O2', '-ftls-model=initial-exec',
                    str(source / 'LateStaticTlsProbe.c'), '-o', str(staging / 'late.so')], check=True)
    shutil.copyfile(staging / 'late.so', staging / 'child.so')
    shutil.copyfile(source / 'LateStaticTlsProbe.py', staging / 'probe.py')
    with InitPool(args.root, args.dist, args.output, size=1, timeout=90) as pool:
        command = ['/usr/bin/python3.11', guest + '/probe.py', guest]
        expected = ['LATE_STATIC_TLS_THREADS_FORK_OK']
        if args.libgomp:
            command.append(args.libgomp)
            expected.append('LIBGOMP_PARALLEL_OK')
        row = pool.run(command, expect=expected)
    (args.output / 'results.json').write_text(json.dumps(row, indent=2), encoding='utf-8')
    print(json.dumps(row))
    return int(row['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
