"""Real command behavior in disposable directories, with independent results."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run(*args, input=None, status=0):
    result = subprocess.run(args, input=input, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=25)
    if result.returncode != status:
        raise AssertionError((args, result.returncode, result.stdout[-4096:], result.stderr[-4096:]))
    return result.stdout


def build(tool):
    Path('a.c').write_text('int value(void) { return 42; }\n')
    Path('b.c').write_text('#include <stdio.h>\nint value(void);\nint main(void){printf("%d\\n",value());}\n')
    if tool == 'make':
        Path('Makefile').write_text('all: result\na.o: a.c\n\t/usr/bin/gcc -c a.c -o a.o\n'
                                   'b.o: b.c\n\t/usr/bin/gcc -c b.c -o b.o\n'
                                   'result: a.o b.o\n\t/usr/bin/gcc a.o b.o -o result\n')
        command = ('/usr/bin/make', '-j2')
    else:
        Path('build.ninja').write_text('rule cc\n  command = /usr/bin/gcc -c $in -o $out\n'
                                      'rule link\n  command = /usr/bin/gcc $in -o $out\n'
                                      'build a.o: cc a.c\nbuild b.o: cc b.c\n'
                                      'build result: link a.o b.o\ndefault result\n')
        command = ('/usr/bin/ninja', '-j2')
    run(*command)
    assert run('./result') == b'42\n'
    before = Path('result').stat().st_mtime_ns
    second = run(*command)
    assert Path('result').stat().st_mtime_ns == before, (second, Path('.ninja_log').read_text() if tool == 'ninja' else '')


def pkgconf():
    Path('example.pc').write_text('prefix=/opt/example\nName: example\nDescription: acceptance fixture\n'
                                  'Version: 1.2.3\nLibs: -L${prefix}/lib -lexample\nCflags: -I${prefix}/include\n')
    command = ('/usr/bin/pkgconf', '--with-path=' + os.getcwd())
    assert run(*command, '--modversion', 'example').strip() == b'1.2.3'
    assert b'-I/opt/example/include' in run(*command, '--cflags', 'example')
    assert b'-lexample' in run(*command, '--libs', 'example')
    run(*command, '--exists', 'no-such-acceptance-package', status=1)


def git():
    run('/usr/bin/git', 'init', '--initial-branch=main', 'source')
    command = ('/usr/bin/git', '-C', 'source')
    run(*command, 'config', 'user.name', 'Acceptance')
    run(*command, 'config', 'user.email', 'acceptance@example.invalid')
    Path('source/file.txt').write_text('committed\n')
    run(*command, 'add', 'file.txt')
    run(*command, 'commit', '-m', 'acceptance fixture')
    assert run(*command, 'show', 'HEAD:file.txt') == b'committed\n'
    Path('source/file.txt').write_text('modified\n')
    assert b'+modified' in run(*command, 'diff', '--', 'file.txt')
    run(*command, 'fsck', '--full')


def git_transfer():
    git()
    run('/usr/bin/git', 'clone', '--no-local', 'source', 'copy')
    assert Path('copy/file.txt').read_text() == 'committed\n'
    run('/usr/bin/git', '-C', 'copy', 'fsck', '--full')

    # Force pack/delta work as well as the minimal packet-reader stack frame.
    # Similar, incompressible blobs yield real deltas without a large fixture.
    command = ('/usr/bin/git', '-C', 'source')
    payload = b''.join(hashlib.sha256(str(i).encode()).digest() for i in range(2048))
    expected = {}
    for revision in range(4):
        for index in range(12):
            name = f'blob-{index:02}.bin'
            content = bytearray(payload)
            offset = (revision * 977 + index * 101) % (len(content) - 32)
            content[offset:offset + 32] = hashlib.sha256(f'{revision}/{index}'.encode()).digest()
            Path('source', name).write_bytes(content)
            expected[name] = hashlib.sha256(content).hexdigest()
        run(*command, 'add', '*.bin')
        run(*command, 'commit', '-m', f'delta revision {revision}')
    head = run(*command, 'rev-parse', 'HEAD')
    for attempt in range(3):
        destination = f'packed-copy-{attempt}'
        run('/usr/bin/git', '-c', 'pack.threads=4', 'clone', '--no-local', 'source', destination)
        copied = ('/usr/bin/git', '-C', destination)
        assert run(*copied, 'rev-parse', 'HEAD') == head
        run(*copied, 'fsck', '--full')
        for name, checksum in expected.items():
            assert hashlib.sha256(Path(destination, name).read_bytes()).hexdigest() == checksum
        indices = list(Path(destination, '.git/objects/pack').glob('*.idx'))
        assert len(indices) == 1, indices
        objects = run('/usr/bin/git', 'verify-pack', '-v', str(indices[0])).splitlines()
        # A deltified object's record includes depth and base object ID.
        assert any(len(row.split()) == 7 and len(row.split()[0]) == 40 for row in objects), objects[-5:]
    print('GIT_TRANSFER_CLONES_4_DELTAS_FSCK_OK')


def text(tool):
    Path('input.txt').write_text('alpha=1\nbeta=2\nalpha=3\n')
    if tool == 'sed':
        run('/bin/sed', '-i', '-E', r's/alpha=([0-9]+)/value=\1/', 'input.txt')
        assert Path('input.txt').read_text() == 'value=1\nbeta=2\nvalue=3\n'
    elif tool == 'grep':
        assert run('/bin/grep', '-E', '^alpha=[13]$', 'input.txt') == b'alpha=1\nalpha=3\n'
        run('/bin/grep', '-q', 'absent', 'input.txt', status=1)
    elif tool == 'find':
        Path('nested').mkdir()
        Path('nested/file with spaces.txt').write_text('x')
        records = run('/usr/bin/find', '.', '-type', 'f', '-name', '*.txt', '-print0').split(b'\0')
        assert set(records) == {b'./input.txt', b'./nested/file with spaces.txt', b''}
        assert run('/usr/bin/xargs', '-0', '/bin/busybox', 'printf', '<%s>',
                   input=b'a b\0c\0') == b'<a b><c>'
    else:
        delta = b'--- input.txt\n+++ input.txt\n@@ -1,3 +1,3 @@\n alpha=1\n-beta=2\n+beta=42\n alpha=3\n'
        run('/usr/bin/patch', '-p0', input=delta)
        assert Path('input.txt').read_text() == 'alpha=1\nbeta=42\nalpha=3\n'


def jq():
    result = run('/usr/bin/jq', '-c', '{total: ([.items[].value] | add), names: [.items[].name]}',
                 input=b'{"items":[{"name":"alpha","value":3},{"name":"beta","value":4}]}')
    assert json.loads(result) == {'total': 7, 'names': ['alpha', 'beta']}
    math = run('/usr/bin/jq', '-n', '[(8|logb),(6|significand),(0|j0),(0|j1),(3|exp10)]')
    assert json.loads(math) == [3, 1.5, 1, 0, 1000]


def openssl():
    payload = bytes(range(256)) * 4096
    Path('payload').write_bytes(payload)
    assert run('/usr/bin/openssl', 'dgst', '-sha256', '-binary', 'payload') == hashlib.sha256(payload).digest()
    encoded = run('/usr/bin/openssl', 'base64', '-A', input=payload[:4096])
    assert run('/usr/bin/openssl', 'base64', '-A', '-d', input=encoded) == payload[:4096]
    assert len(run('/usr/bin/openssl', 'rand', '32')) == 32


def rsync():
    Path('source/nested').mkdir(parents=True)
    payload = bytes(range(256)) * 8192
    Path('source/nested/payload').write_bytes(payload)
    Path('source/nested/name with spaces').write_text('named\n')
    os.link('source/nested/payload', 'source/hardlink')
    os.symlink('nested/payload', 'source/link')
    run('/usr/bin/rsync', '-aH', '--preallocate', 'source/', 'destination/')
    assert Path('destination/nested/payload').read_bytes() == payload
    assert Path('destination/nested/name with spaces').read_text() == 'named\n'
    assert os.readlink('destination/link') == 'nested/payload'
    assert os.stat('destination/hardlink').st_ino == os.stat('destination/nested/payload').st_ino
    updated = b'changed' + payload[7:]
    Path('source/nested/payload').write_bytes(updated)
    Path('destination/obsolete').write_text('remove\n')
    run('/usr/bin/rsync', '-acH', '--delete', '--preallocate', 'source/', 'destination/')
    assert Path('destination/nested/payload').read_bytes() == updated
    assert not Path('destination/obsolete').exists()


def file():
    Path('text.txt').write_text('plain text\n')
    assert b'ASCII text' in run('/usr/bin/file', '-b', 'text.txt')
    assert b'ELF 64-bit' in run('/usr/bin/file', '-b', '/usr/bin/gcc')


def procps():
    pid = os.getpid()
    result = run('/bin/ps', '-p', str(pid), '-o', 'pid=,ppid=,comm=')
    fields = result.split()
    assert int(fields[0]) == pid and int(fields[1]) == os.getppid()
    assert b'python' in fields[2]
    assert b'Mem:' in run('/usr/bin/free', '-b')


name = sys.argv[1]
with tempfile.TemporaryDirectory(prefix='kinakaze-common-tools-') as directory:
    os.chdir(directory)
    if name in ('make', 'ninja'):
        build(name)
    elif name in ('sed', 'grep', 'find', 'patch'):
        text(name)
    else:
        globals()[name]()
    os.chdir('/')
print('COMMON_TOOL_' + name.upper() + '_OK')
