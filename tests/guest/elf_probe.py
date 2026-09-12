"""Shared build/launch/report path for small freestanding Linux ABI regressions."""
from distribution import runtime_image
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

WORKSPACE = Path(__file__).resolve().parents[2]
NO_WINDOW = getattr(subprocess, 'CREATE_NO_WINDOW', 0)


def checked(command):
    command = [str(arg) for arg in command]
    command[0] = shutil.which(command[0]) or command[0]
    result = subprocess.run(command, cwd=WORKSPACE, timeout=30, creationflags=NO_WINDOW,
                            capture_output=True, text=True, encoding='utf-8', errors='replace')
    if result.stdout:
        print(result.stdout, end='')
    if result.stderr:
        print(result.stderr, end='')
    result.check_returncode()


def build(source, root, dist, libraries=(), cflags=(), ldflags=(), link_dir=None):
    checked(['clang', '--target=x86_64-linux-gnu', '-ffreestanding', '-fno-stack-protector',
             '-fno-builtin', '-fPIE', '-O1', *cflags, '-c', source, '-o', root / 'probe.o'])
    checked(['ld.lld', '-pie', '-z', 'now', '-z', 'relro', '--dynamic-linker',
             '/lib64/ld-linux-x86-64.so.2', '-e', '_start', root / 'probe.o',
             '-L', link_dir or WORKSPACE / 'target/debug/elf-imports', '-l:libc.so.6',
             *[f'-l:{name}' for name in libraries], *ldflags, '-o', root / 'probe'])


def run(source_name, root_name, marker, prepare=None, extra_files=None, libraries=(), cflags=(), ldflags=()):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, default=WORKSPACE / 'dist')
    parser.add_argument('--worker', type=Path, default=WORKSPACE / 'target/debug/worker.exe')
    parser.add_argument('--link-dir', type=Path, default=WORKSPACE / 'target/debug/elf-imports')
    args = parser.parse_args()
    dist, worker = args.dist.resolve(), args.worker.resolve()
    root = WORKSPACE / 'artifacts' / root_name
    for directory in ('etc', 'tmp'):
        (root / directory).mkdir(parents=True, exist_ok=True)
    if prepare:
        prepare(root)
    source = Path(__file__).with_name(source_name)
    build(source, root, dist, libraries, cflags, ldflags, args.link_dir.resolve())
    exit_code, timed_out = None, False
    with (root / 'stdout.log').open('wb') as out, (root / 'stderr.log').open('wb') as err:
        try:
            result = subprocess.run([str(worker), 'run', '--root', str(root), '--dist', str(dist),
                                     '--', '/probe'], stdout=out, stderr=err,
                                    timeout=30, creationflags=NO_WINDOW)
            exit_code = result.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
    output = (root / 'stdout.log').read_text(encoding='utf-8', errors='replace')
    passed = not timed_out and exit_code == 0 and marker in output
    files = {'source': source, 'elf': root / 'probe', 'worker': worker,
             'runtime': runtime_image(dist)}
    files.update({name: dist / path for name, path in (extra_files or {}).items()})
    report = {'status': 'passed' if passed else 'failed', 'exit_code': exit_code,
              'timed_out': timed_out, 'expected_output_found': marker in output,
              **{name + '_sha256': hashlib.sha256(path.read_bytes()).hexdigest() for name, path in files.items()}}
    (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    if not passed:
        print((root / 'stderr.log').read_text(encoding='utf-8', errors='replace'))
        raise SystemExit(f'{source_name}: Linux ELF regression failed ({exit_code}, timeout={timed_out})')
    print(output.strip())
