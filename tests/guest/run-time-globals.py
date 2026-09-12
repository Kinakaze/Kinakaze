"""Real Linux ELF timezone tests, including non-PIE COPY relocation and fork."""
from pathlib import Path
import shutil
import subprocess

WORKSPACE = Path(__file__).resolve().parents[2]


def checked(command):
    command = [str(arg) for arg in command]
    command[0] = shutil.which(command[0]) or command[0]
    return subprocess.run(command, check=True, cwd=WORKSPACE, timeout=30,
                          capture_output=True, text=True,
                          creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0)).stdout


def main():
    root = WORKSPACE / 'artifacts/time-globals-root'
    root.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).with_name('time_globals_probe.c')
    # This link-only DSO gives lld actual data sections to generate COPY against.
    # Its SONAME resolves to the production libc provider when the guest runs.
    # It is never installed as a guest library or used as an implementation.
    stub = root / 'link-only.c'
    stub.write_text('long timezone; int daylight; char *tzname[2];\n' +
                    '\n'.join(f'void {name}(void) {{}}' for name in
                              ('tzset', 'setenv', 'fork', 'waitpid', 'localtime_r',
                               'mktime', 'write', '_exit')), encoding='utf-8')
    checked(['clang', '--target=x86_64-linux-gnu', '-ffreestanding', '-fPIC', '-c',
             stub, '-o', root / 'link-only.o'])
    checked(['ld.lld', '-shared', '-soname', 'libc.so.6', root / 'link-only.o',
             '-o', root / 'link-only.so'])
    for mode in ('got', 'copy'):
        obj = root / f'{mode}.o'
        checked(['clang', '--target=x86_64-linux-gnu', '-ffreestanding',
                 '-fno-stack-protector', '-fno-builtin', '-O1',
                 *(['-fPIC'] if mode == 'got' else ['-fno-pic', '-DCOPY_PROBE']),
                 '-c', source, '-o', obj])
        executable = root / mode
        checked(['ld.lld', *(['-pie'] if mode == 'got' else []), '-z', 'now',
                 '--dynamic-linker', '/lib64/ld-linux-x86-64.so.2', '-e', '_start',
                 obj, root / 'link-only.so' if mode == 'copy' else WORKSPACE / 'target/debug/elf-imports/libc.so.6',
                 '-o', executable])
        relocations = checked(['llvm-readobj', '--relocations', executable])
        (root / f'{mode}.relocations.txt').write_text(relocations, encoding='utf-8')
        expected = 'R_X86_64_COPY' if mode == 'copy' else 'R_X86_64_GLOB_DAT'
        for name in ('timezone', 'daylight', 'tzname'):
            if not any(expected in row and name in row for row in relocations.splitlines()):
                raise SystemExit(f'{mode}: missing {expected} for {name}')
        with (root / f'{mode}.stdout.log').open('wb') as out, (root / f'{mode}.stderr.log').open('wb') as err:
            result = subprocess.run([str(WORKSPACE / 'target/debug/worker.exe'),
                                     'run', '--root', str(root), '--dist', str(WORKSPACE / 'dist'),
                                     '--', f'/{mode}'], stdout=out, stderr=err,
                                    timeout=30, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
        output = (root / f'{mode}.stdout.log').read_text(encoding='utf-8', errors='replace')
        if result.returncode or 'TIME_GLOBALS_OK' not in output:
            print((root / f'{mode}.stderr.log').read_text(encoding='utf-8', errors='replace'))
            raise SystemExit(f'{mode}: failed ({result.returncode})')
        print(f'{mode}: {output.strip()}')


if __name__ == '__main__':
    main()
