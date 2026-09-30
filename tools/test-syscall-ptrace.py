"""Build isolated native regression tests and stage their exact SONAME imports.

The default suite covers Windows debugging, descriptor/mapping lifetime, raw
syscall contracts, SYSV IPC and generated instruction bridges. DLL staging uses
only compiler artifacts from this invocation, avoiding mixed target graphs.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import tomllib


PACKAGES = ('kinakaze-v2-libc', 'kinakaze-guest-engine', 'kinakaze-vfs', 'kinakaze-kernel')
FILTERS = {
    'kinakaze-kernel': ('sparse_copy::tests', 'windows::fork_mapping_tests', 'windows::wait_tests', 'cow::tests'),
    'kinakaze-v2-libc': ('ptrace::tests', 'fdio::remap::', 'sysadmin::tests', 'sysvipc::tests'),
    'kinakaze-guest-engine': ('execution::traps::tests', 'execution::instruction_trampoline::tests'),
    'kinakaze-vfs': ('fs::object::tests', 'fs::install_tests', 'fs::atomic_create_tests', 'fs::writeback::tests', 'xattr::tests', 'path::root::tests', 'mount::overlay::mounted::tests', 'mount::overlay::native_lookup::', 'mount::overlay::native_metadata::', 'mount::policy::tests'),
}


def execute(command, *, cwd, timeout, env=None):
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, encoding='utf-8', errors='replace')
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
    except subprocess.TimeoutExpired:
        # Killing only the test launcher can leave debugger fixtures or rustc
        # alive. Retain its owning process handle while killing its own tree.
        subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        process.kill()
        stdout, stderr = process.communicate(timeout=15)
        return subprocess.CompletedProcess(command, 124, stdout, stderr + '\nTest/build deadline exceeded.\n')


def run(root, arguments):
    if os.name != 'nt':
        raise SystemExit('These native debug tests require Windows.')
    command = ['cargo', 'test', '--locked', '--lib', '--no-run', '--message-format=json',
               '--target-dir', str(arguments.target_dir)]
    for package in PACKAGES:
        command += ['-p', package]
    if arguments.profile == 'release':
        command += ['--release']
    build = execute(command, cwd=root, timeout=600)
    print(build.stderr, end='')
    if build.returncode:
        for line in build.stdout.splitlines():
            if line.startswith('{'):
                diagnostic = json.loads(line)
                if diagnostic.get('reason') == 'compiler-message':
                    print(diagnostic['message'].get('rendered', diagnostic['message']['message']), end='')
        raise SystemExit(build.returncode)
    executables = {}
    libraries = {}
    for line in build.stdout.splitlines():
        if not line.startswith('{'):
            continue
        artifact = json.loads(line)
        if artifact.get('reason') != 'compiler-artifact':
            continue
        target = artifact['target']
        if artifact.get('executable') and artifact['profile']['test']:
            name = next(package for package in PACKAGES if package in artifact['package_id'])
            executables[name] = Path(artifact['executable'])
        for filename in artifact['filenames']:
            if filename.endswith('.dll'):
                libraries[target['name']] = Path(filename)
    if set(executables) != set(PACKAGES):
        raise RuntimeError(f'Missing test executables: {executables}')
    aliases = {}
    for directory in [*(root / 'libs').iterdir(), *(root / 'engine/crates').iterdir()]:
        definition = directory / 'exports.def'
        cargo = directory / 'Cargo.toml'
        if not definition.is_file() or not cargo.is_file():
            continue
        metadata = tomllib.loads(cargo.read_text(encoding='utf-8'))
        name = metadata.get('lib', {}).get('name', metadata['package']['name'].replace('-', '_'))
        match = re.search(r'^LIBRARY\s+"?([^"\s]+)', definition.read_text(encoding='utf-8'), re.M)
        if name in libraries and match:
            aliases[name] = match[1]
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    records = []
    output = arguments.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='ptrace-runtime-', dir=output.parent) as staging:
        staging = Path(staging)
        for name, library in libraries.items():
            shutil.copy2(library, staging / aliases.get(name, library.name))
        for library in (sysroot / 'bin').glob('std-*.dll'):
            shutil.copy2(library, staging / library.name)
        environment = dict(os.environ)
        environment['PATH'] = str(staging) + os.pathsep + environment['PATH']
        for package in PACKAGES:
            filters = ('',) if arguments.suite == 'all' else FILTERS[package]
            if arguments.benchmark:
                filters = ('ptrace::tests::benchmark_untraced_syscalls',) if package == 'kinakaze-v2-libc' else ()
            for test_filter in filters:
                command = [str(executables[package]), '--test-threads=1', '--nocapture']
                if arguments.benchmark:
                    command += ['--ignored', '--exact']
                if test_filter:
                    command += [test_filter]
                started = time.monotonic()
                completed = execute(command, cwd=root, env=environment, timeout=180)
                record = dict(package=package, filter=test_filter, exit_code=completed.returncode,
                              seconds=time.monotonic() - started,
                              stdout=completed.stdout, stderr=completed.stderr)
                records.append(record)
                print(f"{package} {test_filter or 'all'}: exit {completed.returncode}")
                print(completed.stdout, end='')
                if completed.stderr:
                    print(completed.stderr, end='')
    report = dict(profile=arguments.profile, suite=arguments.suite,
                  executable_artifacts={key: str(value) for key, value in executables.items()},
                  scope=__doc__, tests=records)
    output.write_text(json.dumps(report, indent=2), encoding='utf-8')
    if any(record['exit_code'] for record in records):
        raise SystemExit(1)
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('debug', 'release'), default='debug')
    parser.add_argument('--suite', choices=('focused', 'all'), default='focused')
    parser.add_argument('--benchmark', action='store_true', help='Run the ignored untraced ABI benchmark (use --profile release).')
    parser.add_argument('--target-dir', type=Path, default=Path('target/syscall-ptrace'))
    parser.add_argument('--output', type=Path, default=Path('artifacts/syscall-ptrace-tests.json'))
    options = parser.parse_args()
    run(Path(__file__).resolve().parents[1], options)
