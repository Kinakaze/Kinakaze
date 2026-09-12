"""Keep two independent managers alive and verify mount/abstract-socket isolation."""
from distribution import runtime_image
import argparse
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time

from elf_probe import WORKSPACE, NO_WINDOW, build

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=WORKSPACE / 'dist')
parser.add_argument('--worker', type=Path, default=WORKSPACE / 'target/debug/worker.exe')
parser.add_argument('--shared-root', action='store_true', help='Use the same host root in both independent managers')
args = parser.parse_args()
dist, worker = args.dist.resolve(), args.worker.resolve()
base = WORKSPACE / ('artifacts/domain-isolation-shared' if args.shared_root else 'artifacts/domain-isolation')
source = Path(__file__).with_name('domain_isolation_probe.c')
processes = []
failure = None
with ExitStack() as stack:
    try:
        for mode in ('a', 'b'):
            root = base / ('root' if args.shared_root else mode)
            for directory in ('source', 'target', 'etc', 'tmp'):
                (root / directory).mkdir(parents=True, exist_ok=True)
            for name in (f'ready-{mode}', 'finish'):
                (root / name).unlink(missing_ok=True)
            (root / 'mode').write_text(mode, encoding='ascii')
            if mode == 'a':
                build(source, root, dist)
            elif not args.shared_root:
                shutil.copyfile(base / 'a/probe', root / 'probe')
            output_path, error_path = root / f'{mode}.stdout.log', root / f'{mode}.stderr.log'
            stdout = stack.enter_context(output_path.open('wb'))
            stderr = stack.enter_context(error_path.open('wb'))
            process = subprocess.Popen([str(worker), 'run', '--root', str(root), '--dist', str(dist),
                                        '--', '/probe'], stdout=stdout, stderr=stderr, creationflags=NO_WINDOW)
            processes.append((process, root, output_path))
            deadline = time.monotonic() + 20
            while not (root / f'ready-{mode}').exists():
                if process.poll() is not None:
                    raise RuntimeError(f'domain {mode} exited {process.returncode}: '
                                       + error_path.read_text(errors='replace'))
                if time.monotonic() >= deadline:
                    raise TimeoutError(f'domain {mode} readiness')
                time.sleep(0.02)
        for process, root, _ in processes:
            (root / 'finish').write_bytes(b'finish\n')
        for process, root, output_path in processes:
            if process.wait(timeout=20) != 0:
                raise RuntimeError(f'{root.name} exit {process.returncode}')
            if b'DOMAIN_ISOLATION_OK' not in output_path.read_bytes():
                raise RuntimeError(f'{root.name} completion marker missing')
        # No sibling retains this namespace: the exec handoff itself must pin it.
        result = subprocess.run([str(worker), 'run', '--root', str(root), '--dist', str(dist),
                                 '--', '/probe', 'exec-owner'], capture_output=True,
                                timeout=20, creationflags=NO_WINDOW)
        (base / 'exec-owner.stdout.log').write_bytes(result.stdout)
        (base / 'exec-owner.stderr.log').write_bytes(result.stderr)
        if result.returncode != 0 or b'NETWORK_EXEC_OWNER_OK' not in result.stdout:
            raise RuntimeError('sole network owner exec handoff failed')
    except Exception as error:
        failure = str(error)
    finally:
        for process, root, _ in processes:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)

report = {'status': 'failed' if failure else 'passed', 'error': failure,
          'runtime_sha256': hashlib.sha256((runtime_image(dist)).read_bytes()).hexdigest(),
          'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest()}
(base / 'report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
if failure:
    raise SystemExit(failure)
print('DOMAIN_ISOLATION_OK: mounts, abstract sockets, networking and fork/exec ownership')
