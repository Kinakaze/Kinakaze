"""Directory entry types/inodes and enumeration after an open directory moves."""
import ctypes as c
import errno
import os
from pathlib import Path
import select
import tempfile
import threading

libc = c.CDLL(None, use_errno=True)
libc.opendir.argtypes = [c.c_char_p]
libc.opendir.restype = c.c_void_p
libc.readdir.argtypes = [c.c_void_p]
libc.readdir.restype = c.c_void_p
libc.rewinddir.argtypes = [c.c_void_p]
libc.closedir.argtypes = [c.c_void_p]
libc.syscall.restype = c.c_long


def stream_entries(stream):
    result = {}
    while True:
        c.set_errno(0)
        entry = libc.readdir(stream)
        if not entry:
            assert c.get_errno() == 0, c.get_errno()
            return result
        name = c.string_at(entry + 19).decode()
        result[name] = (c.c_ubyte.from_address(entry + 18).value, c.c_uint64.from_address(entry).value)


with tempfile.TemporaryDirectory(prefix='directory-types-') as temporary:
    base = Path(temporary)
    directory = base / 'original'
    directory.mkdir()
    (directory / 'file').write_text('data')
    (directory / 'subdir').mkdir()
    os.symlink('missing', directory / 'symlink')
    os.mkfifo(directory / 'fifo')
    os.link(directory / 'file', directory / 'hardlink')
    # Type checks apply to the opened inode even when no data will be read.
    for path, flags, error in [
        (directory, os.O_RDONLY | os.O_TRUNC, errno.EISDIR),
        (directory, os.O_WRONLY, errno.EISDIR),
        (directory / 'file', os.O_RDONLY | os.O_DIRECTORY, errno.ENOTDIR),
        (directory / 'fifo', os.O_RDONLY | os.O_DIRECTORY, errno.ENOTDIR),
        (directory / 'symlink', os.O_RDONLY | os.O_NOFOLLOW, errno.ELOOP),
    ]:
        try:
            opened = os.open(path, flags)
        except OSError as exception:
            assert exception.errno == error, (path, flags, exception)
        else:
            os.close(opened)
            raise AssertionError(('unexpected open', path, flags))
    expected = dict(file=8, subdir=4, symlink=10, fifo=1, hardlink=8)
    stream = libc.opendir(os.fsencode(directory))
    assert stream, c.get_errno()
    try:
        moved = base / 'moved'
        directory.rename(moved)
        directory.mkdir()
        (directory / 'replacement').touch()
        entries = stream_entries(stream)
        assert {name: kind for name, (kind, _) in entries.items() if name not in ('.', '..')} == expected, entries
        for name in expected:
            assert entries[name][1] == (moved / name).lstat().st_ino, (name, entries[name])
        (moved / 'new').touch()
        libc.rewinddir(stream)
        assert 'new' in stream_entries(stream)
        # Repeated enumeration may reuse type hints, but replacing an entry
        # from another process must invalidate them before the next snapshot.
        child = os.fork()
        if not child:
            try:
                os.unlink(moved / 'new')
                os.symlink('subdir', moved / 'new')
            except BaseException:
                os._exit(1)
            os._exit(0)
        assert os.waitpid(child, 0) == (child, 0)
        libc.rewinddir(stream)
        assert stream_entries(stream)['new'][0] == 10
    finally:
        assert libc.closedir(stream) == 0
    descriptor = os.open(moved, os.O_RDONLY | os.O_DIRECTORY)
    try:
        buffer = c.create_string_buffer(4096)
        raw = {}
        while True:
            length = libc.syscall(c.c_long(217), c.c_int(descriptor), c.byref(buffer), c.c_size_t(len(buffer)))
            assert length >= 0, c.get_errno()
            if not length:
                break
            offset = 0
            while offset < length:
                address = c.addressof(buffer) + offset
                size = c.c_uint16.from_address(address + 16).value
                assert size >= 24 and offset + size <= length
                name = c.string_at(address + 19).decode()
                raw[name] = c.c_ubyte.from_address(address + 18).value
                offset += size
        for name, kind in expected.items():
            assert raw[name] == kind, raw
    finally:
        os.close(descriptor)
    stopped = threading.Event()
    failures = []
    def scan():
        try:
            while not stopped.is_set():
                with os.scandir(moved) as stream:
                    for entry in stream:
                        entry.is_file(follow_symlinks=False)
        except BaseException as error:
            failures.append(str(error))
    scanner = threading.Thread(target=scan)
    scanner.start()
    try:
        for _ in range(12):
            read, write = os.pipe()
            child = os.fork()
            if not child:
                try:
                    os.close(read)
                    with os.scandir(moved) as stream:
                        assert any(entry.is_symlink() for entry in stream)
                    os.write(write, b'1')
                except BaseException:
                    os._exit(1)
                os._exit(0)
            os.close(write)
            try:
                if not select.select([read], [], [], 5)[0]:
                    os.kill(child, 9)
                    os.waitpid(child, 0)
                    raise AssertionError('fork child stalled during directory enumeration')
                assert os.read(read, 1) == b'1'
                assert os.waitpid(child, 0) == (child, 0)
            finally:
                os.close(read)
    finally:
        stopped.set()
        scanner.join(timeout=5)
    assert not scanner.is_alive() and not failures, failures
print('DIRECTORY_TYPES_OK', flush=True)
