"""Test a real preinstalled AstrBot checkout and Linux Python, preserving logs."""
import argparse
import json
from pathlib import Path
import uuid

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--python', default='/opt/python312/bin/python3.12')
    parser.add_argument('--app', default='/opt/astrbot')
    parser.add_argument('--pythonpath', default='/opt/astrbot-deps')
    args = parser.parse_args()
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    guest = '/root/astrbot-probe-' + uuid.uuid4().hex
    source = (Path(__file__).resolve().parents[1] / 'tests/guest/AstrbotRuntimeProbe.py').read_text()
    result = dict(dist=str(dist), root=str(root), python=args.python, app=args.app,
                  fixture=guest, sha256=distribution_hashes(dist), status='failed')
    try:
        with InitPool(root, dist, output, size=1, timeout=450) as pool:
            result['probe'] = pool.run([args.python, '-c', source, args.app, guest],
                environment=['PATH=/usr/local/bin:/usr/bin:/bin', 'HOME=/root', 'LANG=C.UTF-8',
                             'TZ=UTC', 'PYTHONPATH=' + args.pythonpath],
                expect=['ASTRBOT_API_AUTH_RESTART_OK'])
            result['status'] = result['probe']['status']
    except Exception as error:
        result['error'] = str(error)
    (output / 'results.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result, indent=2))
    return int(result['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
