"""Run the raw wait ABI probe inside a real Debian guest worker."""
import argparse
import json
from pathlib import Path

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    source = (Path(__file__).resolve().parents[1] / 'tests/guest/SyscallWaitProbe.py').read_text()
    with InitPool(args.root, args.dist, args.output, size=1, timeout=60) as pool:
        row = pool.run(['/usr/bin/python3', '-c', source], expect=['SYSCALL_WAIT_PASS'])
    row['distribution_sha256'] = distribution_hashes(args.dist)
    (args.output / 'report.json').write_text(json.dumps(row, indent=2), encoding='utf-8')
    print(json.dumps(dict(status=row['status'], milliseconds=row['request_to_exit_ms'])))
    return int(row['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
