"""Exercise real ld.so/ldd without running inspected constructors."""
import os
from pathlib import Path
import shutil
import struct
import subprocess

loader = '/lib64/ld-linux-x86-64.so.2'
environment = dict(os.environ, LC_ALL='C')
environment.pop('LD_LIBRARY_PATH', None)
environment.pop('LD_TRACE_LOADED_OBJECTS', None)
count = 0


def run(argv, status=0, env=None):
    global count
    result = subprocess.run(argv, env=env or environment, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
    assert result.returncode == status, (argv, result.returncode, result.stdout, result.stderr)
    count += 1
    return result.stdout


def untouched():
    assert not Path('constructor-ran').exists(), 'inspection ran a constructor'


for directory in ('left', 'right'):
    Path(directory).mkdir(exist_ok=True)
Path('dependency.c').write_text('''#include <fcntl.h>
#include <unistd.h>
__attribute__((constructor)) static void init(void) {
    int fd = open("constructor-ran", O_WRONLY|O_CREAT|O_TRUNC, 0600);
    if (fd >= 0) close(fd);
}
int value(void) { return VALUE; }
''')
Path('program.c').write_text('''#include <stdio.h>
extern int value(void);
int main(int argc, char **argv) { printf("%s:%d\\n", argv[0], value()); return 0; }
''')
for directory, value in [('left', 17), ('right', 42)]:
    run(['gcc', '-fPIC', '-shared', '-DVALUE='+str(value), 'dependency.c',
         '-Wl,-soname,libinterpreter-probe.so', '-o', directory+'/libinterpreter-probe.so'])
run(['gcc', 'program.c', '-Lleft', '-linterpreter-probe', '-Wl,-rpath,$ORIGIN/left', '-o', 'program'])
Path('static.c').write_text('void _start(void) { __asm__ volatile("mov $60,%rax; xor %edi,%edi; syscall"); }')
run(['gcc', '-nostdlib', '-static', '-fno-stack-protector', 'static.c', '-o', 'static'])
Path('not-elf').write_text('this is not an executable\n')
Path('not-elf').chmod(0o755)
assert 'Kinakaze' in run([loader, '--version'])
assert 'Usage:' in run([loader, '--help'])
run(['/bin/sh', '-c', 'exec '+loader+' --verify ./program'])
run([loader, '--verify', './left/libinterpreter-probe.so'], 2)
for target in ('./static', './not-elf', './missing'):
    run([loader, '--verify', target], 1)
for args in (['--unknown'], ['--library-path'], []):
    run([loader]+args, 1)
untouched()
assert 'libinterpreter-probe.so' in run([loader, '--list', './program'])
assert '/left/libinterpreter-probe.so' in run(['/usr/bin/ldd', './program'])
assert '/right/libinterpreter-probe.so' in run([loader, '--list', '--library-path', str(Path('right').resolve()), './program'])
assert '/right/libinterpreter-probe.so' in run([loader, '--list', '--library-path', 'right', './program'])
assert '/left/libinterpreter-probe.so' in run(['./program'], env=dict(environment, LD_TRACE_LOADED_OBJECTS='1'))
untouched()
assert run([loader, '--argv0', 'custom-name', './program']).strip() == 'custom-name:17'
assert Path('constructor-ran').exists()
Path('constructor-ran').unlink()
assert run([loader, '--library-path', str(Path('right').resolve()), './program']).strip() == './program:42'
Path('constructor-ran').unlink()
assert run(['./program'], env=dict(environment, LD_LIBRARY_PATH=str(Path('right').resolve()))).strip() == './program:42'
Path('constructor-ran').unlink()
shutil.copyfile(loader, 'interpreter-copy')
Path('interpreter-copy').chmod(0o755)
run(['./interpreter-copy', '--verify', './program'])
# A rebuilt interpreter facade is executed by the active runtime, even if its
# COFF timestamp differs from the rootfs copy. No host DLL code is loaded here.
image = bytearray(Path(loader).read_bytes())
assert image[:2] == b'MZ'
pe = struct.unpack_from('<I', image, 0x3c)[0]
assert image[pe:pe+4] == b'PE\0\0'
image[pe+8:pe+12] = bytes(value ^ 0xff for value in image[pe+8:pe+12])
Path('interpreter-rebuilt').write_bytes(image)
Path('interpreter-rebuilt').chmod(0o755)
assert 'Kinakaze' in run(['./interpreter-rebuilt', '--version'])
run(['./interpreter-rebuilt', '--verify', './program'])

def rejected(path):
    global count
    Path(path).chmod(0o755)
    try:
        result = subprocess.run(['./'+path, '--version'], stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=20)
        assert result.returncode != 0, (path, result.stdout, result.stderr)
    except OSError as error:
        import errno
        assert error.errno == errno.ENOEXEC, (path, error)
    count += 1

marker = b'kinakaze_process_dlopen\0'
assert marker in image
Path('interpreter-missing-abi').write_bytes(image.replace(marker, b'kinakaze_process_dlopem\0'))
rejected('interpreter-missing-abi')
Path('interpreter-truncated').write_bytes(image[:pe+24])
rejected('interpreter-truncated')
Path('pretend').mkdir()
shutil.copyfile('/lib/libc.so.6', 'pretend/ld-linux-x86-64.so.2')
Path('pretend/ld-linux-x86-64.so.2').chmod(0o755)
try:
    result = subprocess.run(['./pretend/ld-linux-x86-64.so.2', '--version'],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
    assert result.returncode != 0, 'an unrelated PE was accepted as the interpreter'
except OSError as error:
    import errno
    assert error.errno == errno.ENOEXEC, error
count += 1
Path('left/libinterpreter-probe.so').rename('left/hidden.so')
run([loader, '--list', './program'], 1)
untouched()
assert 'libc.so.6' in run(['/usr/bin/ldd', '/bin/cp'])
assert run(['/bin/hostname', '--fqdn']).strip()
# A successful following child must also remain waitable after the query.
run(['/bin/sh', '-ec', 'name=$(hostname --fqdn); test -n "$name"; /bin/true'])
print('INTERPRETER_COMMAND_OK', count, flush=True)
