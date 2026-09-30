"""Check pinned extra agent CLIs in Kinakaze, retaining every startup failure.

This checks Linux CLI startup and option parsing, not model or tool workflows.
Install each Linux CLI and its dependencies in the supplied guest root first.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import uuid

from init_pool import InitPool, distribution_hashes
from agent_resource_watch import AgentResourceWatch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--agent', choices=('opencode', 'zcode', 'dsh'), required=True)
    parser.add_argument('--cli', required=True, help='absolute Linux executable or script')
    parser.add_argument('--node', default='/root/node-v22.23.3-linux-x64/bin/node')
    parser.add_argument('--timeout', type=float, default=45)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    if output.exists():
        parser.error('--output must be new to preserve previous failures')
    cli = root / args.cli.lstrip('/')
    if not args.cli.startswith('/') or not cli.is_file():
        parser.error('--cli must name an installed absolute guest path')
    output.mkdir(parents=True)
    guest = '/tmp/extra-agent-startup-' + uuid.uuid4().hex
    home = root / guest.lstrip('/') / 'home'
    for directory in ('', '.config', '.cache', '.local/share', '.local/state'):
        (home / directory).mkdir(parents=True, exist_ok=True)
    environment = [f'HOME={guest}/home', f'XDG_CONFIG_HOME={guest}/home/.config',
                   f'XDG_CACHE_HOME={guest}/home/.cache',
                   f'XDG_DATA_HOME={guest}/home/.local/share',
                   f'XDG_STATE_HOME={guest}/home/.local/state',
                   f'PATH={str(Path(args.node).parent).replace(chr(92), "/")}:/usr/bin:/bin',
                   'LANG=C.UTF-8', 'TERM=dumb', 'NO_COLOR=1',
                   'OPENCODE_DISABLE_AUTOUPDATE=1', 'OPENCODE_DISABLE_MODELS_FETCH=1']
    executable = [args.node, args.cli] if args.agent == 'dsh' else [args.cli]
    cases = [('version', ['--version']), ('help', ['--help'])]
    if args.agent == 'dsh':
        cases.extend([('headless-help', ['--profile', 'headless', '--help']),
                      ('headless-config', ['--profile', 'headless', '--dump-config'])])
    report = dict(scope=__doc__, agent=args.agent, root=str(root), dist=str(dist),
                  guest=guest, sha256=distribution_hashes(dist),
                  cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest(),
                  harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  results=[], passed=False)

    def save():
        report['passed'] = (len(report['results']) == len(cases)
                            and all(row['passed'] for row in report['results']))
        (output / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')

    save()
    for name, options in cases:
        case = output / name
        row = dict(case=name, command=executable + options, passed=False)
        watcher = None
        try:
            with InitPool(root, dist, case, timeout=args.timeout, size=1,
                          memory_limit_bytes=8 * 1024**3) as pool:
                watcher = AgentResourceWatch(pool)
                row['run'] = pool.run(['/usr/bin/env', '-i', *environment, *executable, *options], cwd=guest)
        except Exception as error:
            row['error'] = str(error)
            row['timed_out'] = pool.timed_out.is_set() if 'pool' in locals() else False
        finally:
            if watcher:
                row['resources'] = watcher.finish()
        for stream in ('stdout', 'stderr'):
            log = case / f'pool.{stream}.log'
            row[stream] = log.read_text(encoding='utf-8', errors='replace') if log.is_file() else ''
        text = row['stdout'] + row['stderr']
        if name == 'version':
            meaningful = bool(re.search(r'\d+\.\d+\.\d+', text))
        elif name.endswith('help'):
            meaningful = 'usage' in text.lower() and args.agent in text.lower()
        else:
            meaningful = 'headless' in text and ('insert' in text or 'plugin' in text)
        row['passed'] = (row.get('run', {}).get('status') == 'passed' and meaningful
                         and row.get('resources', {}).get('cleanup_passed', False)
                         and not row.get('resources', {}).get('limit_exceeded'))
        report['results'].append(row)
        save()
        print(json.dumps(dict(agent=args.agent, case=name, passed=row['passed'],
                              exit_code=row.get('run', {}).get('exit_code'), error=row.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
