"""Native readonly mode, DAC checks, and opens pinned through procfs."""
import errno
import os
from pathlib import Path
import subprocess
import sys
import tempfile


with tempfile.TemporaryDirectory(prefix='native-permissions-') as temporary:
    directory = Path(temporary)
    os.chmod(directory, 0o755)
    path = directory / 'readonly'
    path.write_bytes(b'original')
    os.chmod(path, 0o444)
    # root must obtain a writable open without changing Linux mode or identity.
    before = path.stat()
    fd = os.open(path, os.O_RDWR | os.O_TRUNC)
    try:
        os.write(fd, b'root')
        assert os.fstat(fd).st_ino == before.st_ino
    finally:
        os.close(fd)
    assert path.read_bytes() == b'root'
    assert path.stat().st_mode & 0o777 == 0o444
    pin = os.open(path, os.O_PATH)
    try:
        renamed = directory / 'renamed'
        path.rename(renamed)
        fd = os.open(f'/proc/self/fd/{pin}', os.O_RDWR)
        try:
            assert os.fstat(fd).st_ino == before.st_ino
            os.write(fd, b'pin!')
        finally:
            os.close(fd)
        assert renamed.read_bytes() == b'pin!'
    finally:
        os.close(pin)
    script = '''import errno,os,sys
os.setgroups([]); os.setgid(65534); os.setuid(65534)
for flags in (os.O_WRONLY, os.O_RDWR, os.O_WRONLY|os.O_TRUNC):
    try: fd=os.open(sys.argv[1],flags)
    except OSError as error: assert error.errno==errno.EACCES,error
    else: os.close(fd); raise AssertionError('non-owner obtained write access')
with open(sys.argv[1],'rb') as stream: assert stream.read()==b'pin!'
'''
    subprocess.run([sys.executable, '-c', script, str(renamed)], check=True, timeout=10)
    assert renamed.read_bytes() == b'pin!'
    # Owner-write must not substitute for the permissions of another uid.
    os.chmod(renamed, 0o644)
    subprocess.run([sys.executable, '-c', script, str(renamed)], check=True, timeout=10)
    assert renamed.read_bytes() == b'pin!'
    # Group/other write permission remains usable even without owner-write.
    os.chmod(renamed, 0o446)
    subprocess.run([sys.executable, '-c', '''import os,sys
os.setgroups([]); os.setgid(65534); os.setuid(65534)
with open(sys.argv[1],'wb') as stream: stream.write(b'other')
''', str(renamed)], check=True, timeout=10)
    assert renamed.read_bytes() == b'other'
    assert renamed.stat().st_mode & 0o777 == 0o446
    # Replacing a directory entry does not require write access to its contents.
    # The old readonly inode must remain readable through existing descriptors.
    for kind in ('file', 'symlink'):
        target = directory / ('target-' + kind)
        source = directory / ('source-' + kind)
        target.write_bytes(b'old inode')
        os.chmod(target, 0o444)
        old = os.open(target, os.O_RDONLY)
        directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            if kind == 'file':
                source.write_bytes(b'replacement')
                os.chmod(source, 0o640)
                os.replace(source, target)
                assert target.read_bytes() == b'replacement'
                assert target.stat().st_mode & 0o777 == 0o640
            else:
                source.symlink_to(renamed.name)
                os.rename(source.name, target.name, src_dir_fd=directory_fd, dst_dir_fd=directory_fd)
                assert target.is_symlink() and target.read_bytes() == b'other'
            assert os.read(old, 32) == b'old inode'
            assert os.fstat(old).st_mode & 0o777 == 0o444
            assert not source.exists()
        finally:
            os.close(old)
            os.close(directory_fd)
print('NATIVE_PERMISSIONS_OK', flush=True)
