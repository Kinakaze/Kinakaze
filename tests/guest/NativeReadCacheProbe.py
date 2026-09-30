"""Native read capabilities stay on their inode through dup/fork/exec/close."""
import hashlib
import os
import tempfile

payload = bytes(range(256)) * 256
digest = hashlib.sha256(payload).hexdigest()
with tempfile.TemporaryDirectory(prefix='native-read-cache-', dir='/var/tmp') as directory:
    path = directory + '/original'
    with open(path, 'wb') as stream:
        stream.write(payload)
    fd = os.open(path, os.O_RDONLY)
    try:
        os.set_inheritable(fd, True)
        inode = os.fstat(fd).st_ino
        assert os.pread(fd, len(payload), 0) == payload
        os.rename(path, directory + '/renamed')
        os.unlink(directory + '/renamed')
        with open(path, 'wb') as stream:
            stream.write(b'replacement')
        for _ in range(4):
            os.lseek(fd, 17, os.SEEK_SET)
            child = os.fork()
            if child == 0:
                try:
                    alias = os.dup(fd)
                    os.close(fd)
                    assert os.pread(alias, len(payload), 0) == payload
                    assert os.fstat(alias).st_ino == inode
                    assert os.read(alias, 1024) == payload[17:1041]
                    os.close(alias)
                except BaseException:
                    os._exit(1)
                os._exit(0)
            assert os.waitpid(child, 0) == (child, 0)
            assert os.lseek(fd, 0, os.SEEK_CUR) == 1041
            assert os.pread(fd, 100, 1000) == payload[1000:1100]
        child = os.fork()
        if child == 0:
            code = (
                'import hashlib,os,sys; fd=int(sys.argv[1]); '
                'assert hashlib.sha256(os.pread(fd,65536,0)).hexdigest()==sys.argv[2]; '
                'assert os.lseek(fd,0,os.SEEK_CUR)==1041; os.close(fd)'
            )
            os.execv('/usr/bin/python3.11', ['/usr/bin/python3.11', '-c', code, str(fd), digest])
        assert os.waitpid(child, 0) == (child, 0)
        assert os.pread(fd, len(payload), 0) == payload
        with open(path, 'rb') as stream:
            assert stream.read() == b'replacement'
    finally:
        os.close(fd)
print('NATIVE_READ_CACHE_OK', flush=True)
