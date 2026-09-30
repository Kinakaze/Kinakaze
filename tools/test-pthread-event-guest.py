"""Run pthread cleanup and event parking ABI probes in both native switch modes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid

from init_pool import InitPool, distribution_hashes

PROBES = {
    'cleanup': ('PthreadCleanupProbe', ['PTHREAD_C_CLEANUP_LIFECYCLE_OK']),
    'cancel': ('PthreadCondCancelProbe', ['PTHREAD_COND_CANCEL_CLEANUP_OK']),
    'event': ('PthreadEventParkingProbe', ['PTHREAD_EVENT_TIMED_OK',
                                         'PTHREAD_EVENT_SIGNAL_OK', 'PTHREAD_EVENT_FORK_OK']),
    'robust': ('PthreadRobustProbe', ['PTHREAD_ROBUST_MUTEX_OK']),
    'shared': ('PthreadSharedMutexProbe', ['PTHREAD_SHARED_MUTEX_OK']),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--clang', default='clang')
    parser.add_argument('--probe', action='append', choices=PROBES)
    parser.add_argument('--event-case', action='append', choices=('timed', 'signal', 'fork'))
    parser.add_argument('--timeout', type=float, default=120)
    parser.add_argument('--robust-cancel', action='store_true',
                        help='compile the real GNU condition cleanup probe with a robust mutex')
    args = parser.parse_args()
    selected = args.probe or list(PROBES)
    source = Path(__file__).resolve().parents[1] / 'tests/guest'
    guest = '/tmp/kinakaze-pthread-events-' + uuid.uuid4().hex
    staging = args.root.resolve() / guest.lstrip('/')
    staging.mkdir(parents=True)
    args.output.mkdir(parents=True, exist_ok=True)
    inputs = {}
    for key in selected:
        name, _ = PROBES[key]
        for suffix in ('.py', '.c'):
            fixture = source / (name + suffix)
            inputs[fixture.name] = hashlib.sha256(fixture.read_bytes()).hexdigest()
        shutil.copyfile(source / (name + '.py'), staging / (name + '.py'))
        defines = ['-DPROBE_ROBUST_MUTEX'] if key == 'cancel' and args.robust_cancel else []
        subprocess.run([args.clang, '--target=x86_64-linux-gnu', '-fuse-ld=lld',
                        '-fPIC', '-shared', '-nostdlib', '-O2', *defines, str(source / (name + '.c')),
                        '-o', str(staging / (name + '.so'))], check=True)
    report = dict(root=str(args.root.resolve()), dist=str(args.dist.resolve()), robust_cancel=args.robust_cancel,
                  staging=str(staging), fixture_sha256=inputs,
                  distribution_sha256=distribution_hashes(args.dist), rows=[])
    switches = ('KINAKAZE_PTHREAD_PARK_OPT', 'KINAKAZE_PTHREAD_SHARED_CACHE_OPT')
    previous = {name: os.environ.get(name) for name in switches}
    try:
        for enabled in (False, True):
            # A prewarmed worker caches this native switch before guest environ.
            for name in switches:
                os.environ[name] = str(int(enabled))
            for key in selected:
                name, markers = PROBES[key]
                command = ['/usr/bin/python3.11', guest + '/' + name + '.py']
                if key == 'event' and args.event_case:
                    command += args.event_case
                    markers = ['PTHREAD_EVENT_' + case.upper() + '_OK' for case in args.event_case]
                case = args.output / ('candidate' if enabled else 'control') / key
                with InitPool(args.root, args.dist, case, size=1, timeout=args.timeout) as pool:
                    row = pool.run(command, expect=markers,
                                   environment=['PATH=/usr/bin:/bin', 'LC_ALL=C',
                                                *[name + '=' + str(int(enabled)) for name in switches]])
                row.update(probe=key, optimized=enabled)
                report['rows'].append(row)
                print(json.dumps(dict(probe=key, optimized=enabled, status=row['status'],
                                      milliseconds=row['request_to_exit_ms'])), flush=True)
                report['passed'] = all(row['status'] == 'passed' for row in report['rows'])
                (args.output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
