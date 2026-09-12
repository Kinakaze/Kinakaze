"""Build and run a real Linux CUDA driver probe on the host NVIDIA GPU."""
from distribution import runtime_image
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

WORKSPACE = Path(__file__).resolve().parents[2]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, default=WORKSPACE / 'dist')
    parser.add_argument('--worker', type=Path, default=WORKSPACE / 'target/debug/worker.exe')
    parser.add_argument('--fork', action='store_true', help='Also test CUDA ownership across fork')
    parser.add_argument('--ptxas', type=Path, help='Compile PTX to cubin before running (AOT)')
    parser.add_argument('--gpu-architecture', help='Explicit ptxas target, for example sm_89')
    parser.add_argument('--timeout', type=float, default=60)
    args = parser.parse_args()
    if args.ptxas and (not args.gpu_architecture or args.fork):
        parser.error('--ptxas requires --gpu-architecture and a separate run from --fork')
    root = WORKSPACE / 'artifacts/cuda-root'
    root.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).with_name('cuda_probe.c')
    ptx = Path(__file__).with_name('cuda-vector.ptx')
    shutil.copy2(ptx, root / ptx.name)
    (root / 'empty-image').write_bytes(b'')
    if args.ptxas:
        subprocess.run([str(args.ptxas.resolve()), '-arch=' + args.gpu_architecture,
                        str(ptx), '-o', str(root / 'cuda-vector.cubin')], check=True, timeout=30,
                       creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    obj, executable = root / 'probe.o', root / 'probe'
    for command in [
        ['clang', '--target=x86_64-linux-gnu', '-ffreestanding', '-fno-stack-protector',
         '-fno-builtin', '-fPIC', '-O1', '-c', source, '-o', obj],
        ['ld.lld', '-pie', '-z', 'now', '-z', 'relro', '--dynamic-linker', '/lib64/ld-linux-x86-64.so.2',
         '-e', '_start', obj, '-L', WORKSPACE / 'target/debug/elf-imports', '-l:libcuda.so.1', '-l:libc.so.6', '-o', executable],
    ]:
        command = [str(part) for part in command]
        command[0] = shutil.which(command[0]) or command[0]
        subprocess.run(command, check=True, timeout=30, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    mode = 'fork' if args.fork else 'cubin' if args.ptxas else 'compute'
    output_path, error_path = root / f'{mode}.stdout.log', root / f'{mode}.stderr.log'
    command = [str(args.worker.resolve()), 'run', '--root', str(root), '--dist', str(args.dist.resolve()), '--', '/probe']
    if args.fork:
        command.append('--fork')
    if args.ptxas:
        command.append('--cubin')
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), mode=mode,
                  runtime_sha256=digest(runtime_image(args.dist)),
                  worker_sha256=digest(args.worker), source_sha256=digest(source), ptx_sha256=digest(ptx),
                  executable_sha256=digest(executable), stdout=str(output_path), stderr=str(error_path))
    if args.ptxas:
        report.update(ptxas_sha256=digest(args.ptxas), cubin_sha256=digest(root / 'cuda-vector.cubin'),
                      gpu_architecture=args.gpu_architecture)
    with output_path.open('wb') as out, error_path.open('wb') as err:
        try:
            result = subprocess.run(command, stdout=out, stderr=err, timeout=args.timeout,
                                    creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
            report['exit_code'] = result.returncode
            report['status'] = 'passed' if result.returncode == 0 else 'failed'
        except subprocess.TimeoutExpired:
            report['status'] = 'timeout'
    output = output_path.read_text(encoding='utf-8', errors='replace')
    markers = ['CUDA_DRIVER_OK', 'CUDA_PROC_ADDRESS_OK', 'CUDA_KERNEL_STREAM_EVENT_CLEANUP_OK']
    if args.fork:
        markers += ['CUDA_FORK_BEFORE_INIT_OK', 'CUDA_FORK_GUARD_OK', 'CUDA_EXEC_AFTER_FORK_OK']
    report['missing_markers'] = [marker for marker in markers if marker not in output]
    if report['status'] == 'passed' and report['missing_markers']:
        report['status'] = 'failed'
    (root / f'{mode}.report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(output[-4096:])
    if report['status'] != 'passed':
        print(error_path.read_text(encoding='utf-8', errors='replace')[-4096:])
        raise SystemExit(f'CUDA probe {report["status"]}: {report.get("exit_code")}')


if __name__ == '__main__':
    main()
