"""Run owned Linux ABI probes, including real C setjmp and variadic callers."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import time
import uuid

from session_process import SessionProcess

PROBES = ('XVisibilityProbe', 'FutexUnflaggedProbe', 'FutexSharedProbe', 'GLStorageProbe', 'CompiledSwitchProbe', 'GLTexGenProbe', 'MmapHintProbe', 'MixedXlibXcbProbe', 'UdevMonitorProbe', 'NeteaseStartupAbiProbe', 'NeteaseAudioAbiProbe', 'PthreadCleanupProbe', 'PthreadCondCancelProbe', 'WideFormatProbe', 'AlsaCommonInterfacesProbe',
          'CommonLibcAliasesProbe', 'UnixBlockingProbe', 'AllocationLifecycleProbe',
          'PythonRuntimeProbe', 'LineReadProbe', 'UnixPerformanceProbe',
          'UnixRightsProbe', 'UnixCredentialsProbe', 'AllocationGrowthProbe',
          'UnixPeekProbe', 'AlsaSequencerProbe', 'StartupInterfacesProbe', 'Power80Probe', 'PipeWireLoopProbe', 'AllocationRecycleProbe', 'UnixReadinessProbe', 'PipeWireClientProbe', 'AllocationBatchProbe', 'AllocationColdProbe', 'AllocationThreadRecycleProbe')
EXECUTABLES = ('AllocatorInterpositionProbe', 'AllocatorIfuncProbe')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--clang', default='clang')
    parser.add_argument('--elf-imports', type=Path, default=Path('target/release/elf-imports'))
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--probe', action='append', choices=(*PROBES, *EXECUTABLES))
    args = parser.parse_args()
    selected = args.probe or (*PROBES, *EXECUTABLES)
    source = Path(__file__).resolve().parents[1] / 'tests/guest'
    root, dist = args.root.resolve(), args.dist.resolve()
    guest = '/tmp/kinakaze-compat-' + uuid.uuid4().hex
    staging = root / guest.lstrip('/')
    staging.mkdir(parents=True)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    # Retain these small fixtures for reproducing failures. No user tmp files
    # or existing processes are replaced or removed by this runner.
    for name in selected:
        if name in EXECUTABLES:
            continue
        shutil.copyfile(source / (name + '.py'), staging / (name + '.py'))
        fixture = source / (name + '.c')
        if fixture.is_file():
            subprocess.run([args.clang, '--target=x86_64-linux-gnu', '-fuse-ld=lld',
                            '-fPIC', '-shared', '-nostdlib', '-O2', str(fixture),
                            '-o', str(staging / (name + '.so'))], check=True)
    for name in EXECUTABLES:
        if name not in selected:
            continue
        defines = ['-DALLOCATOR_IFUNC'] if name == 'AllocatorIfuncProbe' else []
        subprocess.run([args.clang, '--target=x86_64-linux-gnu', '-fuse-ld=lld',
                        '-fPIE', '-pie', '-nostdlib', '-fno-builtin', '-O2', *defines,
                        '-Wl,--export-dynamic', '-Wl,--dynamic-linker=/lib64/ld-linux-x86-64.so.2',
                        str(source / 'AllocatorInterpositionProbe.c'), '-L' + str(args.elf_imports.resolve()),
                        '-l:libc.so.6', '-l:libpthread.so.0', '-l:libasound.so.2',
                        '-o', str(staging / name)], check=True)
    results = []
    for name in selected:
        logfile = args.output_dir / (name + '.log')
        started = time.perf_counter()
        code, status = None, 'timeout'
        with logfile.open('wb') as log:
            command = ([guest + '/' + name] if name in EXECUTABLES else
                       ['/usr/bin/python3.11', guest + '/' + name + '.py'])
            child = SessionProcess([str(dist/'worker.exe'), 'run', '--root', str(root),
                                    '--dist', str(dist), '--', *command], stdout=log, stderr=log)
            try:
                code = child.process.wait(timeout=args.timeout)
                status = 'passed' if code == 0 else 'failed'
            except subprocess.TimeoutExpired:
                pass
            finally:
                child.close()
        result = dict(probe=name, status=status, exit_code=code,
                      elapsed_seconds=time.perf_counter()-started, log=str(logfile.resolve()))
        results.append(result)
        print(json.dumps(result), flush=True)
        (args.output_dir/'results.json').write_text(json.dumps(dict(
            root=str(root), dist=str(dist), staging=str(staging), results=results), indent=2))
    return int(any(row['status'] != 'passed' for row in results))


if __name__ == '__main__':
    raise SystemExit(main())
