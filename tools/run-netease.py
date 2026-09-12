"""Run the prepared Linux NetEase client with its own profile and D-Bus session."""
import argparse
import json
from pathlib import Path
import subprocess
import time
import uuid

from session_process import SessionProcess

WORKSPACE = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=WORKSPACE / 'artifacts/netease-cloud-music/rootfs')
    parser.add_argument('--dist', type=Path, default=WORKSPACE / 'artifacts/netease-dist')
    parser.add_argument('--timeout', type=float, default=0, help='Stop after this many seconds; zero keeps the app open')
    parser.add_argument('--log-dir', type=Path, default=WORKSPACE / 'artifacts/netease-cloud-music/logs')
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.timeout < 0:
        parser.error('--timeout must be nonnegative')
    root, dist, logs = args.root.resolve(), args.dist.resolve(), args.log_dir.resolve()
    for path in (dist / 'worker.exe', root / 'usr/bin/netease-cloud-music', root / 'usr/bin/dbus-run-session', root / 'bin/busybox'):
        if not path.is_file():
            parser.error('Required prepared file is missing: ' + str(path))
    home = '/home/netease'
    runtime = '/tmp/netease-runtime-' + uuid.uuid4().hex
    for directory in (home, home + '/.config', home + '/.cache', home + '/.local/share', runtime):
        (root / directory.lstrip('/')).mkdir(parents=True, exist_ok=True)
    logs.mkdir(parents=True, exist_ok=True)
    token = 'session-' + time.strftime('%Y%m%d-%H%M%S') + '-' + uuid.uuid4().hex[:8]
    logpath, reportpath = logs / (token + '.log'), logs / (token + '.json')
    arguments = args.arguments[1:] if args.arguments[:1] == ['--'] else args.arguments
    command = [str(dist / 'worker.exe'), 'run', '--root', str(root), '--dist', str(dist), '--',
               '/bin/busybox', 'env', 'HOME=' + home, 'XDG_CONFIG_HOME=' + home + '/.config',
               'XDG_CACHE_HOME=' + home + '/.cache', 'XDG_DATA_HOME=' + home + '/.local/share',
               'XDG_RUNTIME_DIR=' + runtime, 'QT_QPA_PLATFORM=xcb',
               'LD_LIBRARY_PATH=/usr/lib/x86_64-linux-gnu/qcef',
               '/bin/sh', '-c', '/bin/busybox chmod 700 "$XDG_RUNTIME_DIR" && '
               'exec /usr/bin/dbus-run-session -- /usr/bin/netease-cloud-music "$@"',
               'netease', *arguments]
    print('NetEase Linux client log:', logpath, flush=True)
    start, status, code = time.monotonic(), 'exited', None
    with logpath.open('wb') as log:
        session = SessionProcess(command, stdout=log, stderr=log)
        try:
            code = session.process.wait(timeout=args.timeout or None)
        except subprocess.TimeoutExpired:
            status = 'timeout'
        except KeyboardInterrupt:
            status = 'interrupted'
        finally:
            session.close()
            session.process.wait(timeout=10)
    reportpath.write_text(json.dumps(dict(command=command, status=status, exit_code=code,
        elapsed_seconds=round(time.monotonic() - start, 3), log=str(logpath)), indent=2) + '\n', encoding='utf-8')
    print('Session result:', reportpath, flush=True)
    return code if code is not None else 124 if status == 'timeout' else 130


if __name__ == '__main__':
    raise SystemExit(main())
