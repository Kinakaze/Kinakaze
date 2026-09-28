"""Relative statfs, errors, descriptor lifetime and mount flags seen by Debian df."""
import errno
import os
import subprocess
import tempfile

with tempfile.TemporaryDirectory(prefix='statfs-boundary-') as directory:
    previous = os.getcwd()
    try:
        os.chdir(directory)
        absolute = os.statvfs(directory)
        relative = os.statvfs('.')
        assert relative.f_bsize == absolute.f_bsize
        assert relative.f_fsid == absolute.f_fsid
        for path, expected in [('', errno.ENOENT), ('absent', errno.ENOENT),
                               ('absent/child', errno.ENOENT)]:
            try:
                os.statvfs(path)
            except OSError as error:
                assert error.errno == expected, (path, error)
            else:
                raise AssertionError(('statvfs accepted missing path', path))
        with open('file', 'w') as stream:
            stream.write('data')
            try:
                os.statvfs('file/child')
            except OSError as error:
                assert error.errno == errno.ENOTDIR, error
            else:
                raise AssertionError('statvfs accepted a nondirectory parent')
            before = os.fstatvfs(stream.fileno())
            os.unlink('file')
            after = os.fstatvfs(stream.fileno())
            assert before.f_bsize == after.f_bsize and before.f_fsid == after.f_fsid
        os.symlink('/proc', 'proc-link')
        assert os.statvfs('proc-link').f_bsize == os.statvfs('/proc').f_bsize
        subprocess.run(['/bin/df', '.'], check=True, stdout=subprocess.PIPE)
    finally:
        os.chdir(previous)
print('STATFS_BOUNDARIES_OK', flush=True)
