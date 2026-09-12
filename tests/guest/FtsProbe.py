"""Exercise actual FTS traversal, control instructions, ABI and fork state."""
import ctypes as c
import errno
import os
from pathlib import Path
import tempfile


class Entry(c.Structure):
    pass


P = c.POINTER(Entry)
Entry._fields_ = [
    ('cycle', P), ('parent', P), ('link', P), ('number', c.c_long),
    ('pointer', c.c_void_p), ('accpath', c.c_char_p), ('path', c.c_char_p),
    ('error', c.c_int), ('symfd', c.c_int), ('pathlen', c.c_ushort),
    ('namelen', c.c_ushort), ('ino', c.c_ulong), ('dev', c.c_ulong),
    ('nlink', c.c_ulong), ('level', c.c_short), ('info', c.c_ushort),
    ('flags', c.c_ushort), ('instruction', c.c_ushort),
    ('stat', c.c_void_p), ('name', c.c_char * 1),
]
assert Entry.info.offset == 98 and Entry.name.offset == 112
lib = c.CDLL('libc.so.6', use_errno=True)
COMPARE = c.CFUNCTYPE(c.c_int, c.POINTER(P), c.POINTER(P))


def name(entry):
    return c.string_at(c.addressof(entry.contents) + Entry.name.offset)


@COMPARE
def compare(left, right):
    a, b = name(left[0]), name(right[0])
    return (a > b) - (a < b)


def functions(prefix):
    signatures = {
        'open': (c.c_void_p, c.POINTER(c.c_char_p), c.c_int, COMPARE),
        'read': (P, c.c_void_p), 'children': (P, c.c_void_p, c.c_int),
        'set': (c.c_int, c.c_void_p, P, c.c_int), 'close': (c.c_int, c.c_void_p),
    }
    result = {}
    for suffix, signature in signatures.items():
        fn = getattr(lib, prefix + suffix)
        fn.restype, fn.argtypes = signature[0], signature[1:]
        result[suffix] = fn
    return result


def drain(f, stream):
    rows = []
    while entry := f['read'](stream):
        value = entry.contents
        rows.append((value.path, value.info, value.level))
        assert value.pathlen == len(value.path)
        assert value.namelen == len(name(entry))
        if value.info == 1:
            assert os.path.isdir(value.accpath)
    assert c.get_errno() == 0
    return rows


with tempfile.TemporaryDirectory(prefix='fts-probe-') as tmp:
    root = Path(tmp)
    (root / 'a').write_text('a')
    (root / 'b').write_text('b')
    (root / 'skip').mkdir()
    (root / 'skip' / 'hidden').write_text('hidden')
    (root / 'link').symlink_to('a')
    (root / 'loop').symlink_to('.')
    (root / 'missing').symlink_to('absent')
    paths = (c.c_char_p * 2)(os.fsencode(root), None)
    for prefix in ('fts_', 'fts64_'):
        f = functions(prefix)
        assert not f['open'](paths, 0x1000, compare) and c.get_errno() == errno.EINVAL
        stream = f['open'](paths, 4 | 16, compare)
        assert stream
        entry = f['read'](stream)
        assert entry.contents.info == 1 and entry.contents.level == 0
        assert f['set'](stream, entry, 99) == 1 and c.get_errno() == errno.EINVAL
        entry.contents.number = 12345
        assert f['set'](stream, entry, 1) == 0
        again = f['read'](stream)
        assert c.addressof(again.contents) == c.addressof(entry.contents)
        assert again.contents.number == 12345
        child = f['children'](stream, 0)
        child_names = []
        while child:
            child_names.append(name(child))
            if name(child) == b'skip':
                assert f['set'](stream, child, 4) == 0
            child = child.contents.link
        assert child_names == sorted(child_names)
        rows = drain(f, stream)
        assert not any(b'/skip' in path for path, _, _ in rows)
        assert (os.fsencode(root / 'missing'), 12, 1) in rows
        assert rows[-1] == (os.fsencode(root), 6, 0)
        assert f['close'](stream) == 0

        # A current-directory skip still returns its postorder entry.
        stream = f['open'](paths, 4 | 16, compare)
        entry = f['read'](stream)
        assert f['children'](stream, 0)
        assert f['set'](stream, entry, 4) == 0
        assert f['read'](stream).contents.info == 6
        assert not f['read'](stream)
        assert f['close'](stream) == 0

        # Explicitly follow a file and a cyclic directory link during a physical walk.
        stream = f['open'](paths, 4 | 16, compare)
        saw = set()
        while entry := f['read'](stream):
            if name(entry) in (b'link', b'loop', b'missing') and entry.contents.info == 12:
                key = name(entry)
                assert f['set'](stream, entry, 2) == 0
                followed = f['read'](stream)
                assert followed.contents.info == {b'link': 8, b'loop': 2, b'missing': 13}[key]
                if key == b'loop':
                    assert followed.contents.cycle.contents.level == 0
                saw.add(key)
        assert saw == {b'link', b'loop', b'missing'}
        assert f['close'](stream) == 0

        # Cached children and live stream pointers survive fork independently.
        stream = f['open'](paths, 2, compare)
        assert f['read'](stream).contents.info == 1
        assert f['children'](stream, 0x100)
        pid = os.fork()
        if pid == 0:
            try:
                rows = drain(f, stream)
                assert (os.fsencode(root / 'loop'), 2, 1) in rows
                assert f['close'](stream) == 0
                os._exit(0)
            except BaseException:
                os._exit(91)
        rows = drain(f, stream)
        assert (os.fsencode(root / 'loop'), 2, 1) in rows
        assert f['close'](stream) == 0
        assert os.waitpid(pid, 0) == (pid, 0)

        cwd = os.getcwd()
        stream = f['open'](paths, 16, compare)
        assert f['read'](stream).contents.info == 1
        assert f['read'](stream).contents.level == 1
        assert os.getcwd() == str(root)
        assert f['close'](stream) == 0 and os.getcwd() == cwd
        stream = f['open'](paths, 4 | 8 | 16, compare)
        rows = drain(f, stream)
        assert (os.fsencode(root / 'a'), 11, 1) in rows
        assert f['close'](stream) == 0
        empty = (c.c_char_p * 1)(None)
        stream = f['open'](empty, 4, compare)
        assert stream and not f['read'](stream) and c.get_errno() == 0
        assert f['close'](stream) == 0

        parent_pid = os.getpid()
        forked = [False]

        @COMPARE
        def fork_compare(left, right):
            if not forked[0]:
                forked[0] = True
                child = os.fork()
                if child:
                    assert os.waitpid(child, 0) == (child, 0)
            return compare(left, right)

        ordered = (c.c_char_p * 3)(os.fsencode(root / 'b'), os.fsencode(root / 'a'), None)
        stream = f['open'](ordered, 4 | 16, fork_compare)
        rows = drain(f, stream)
        assert [row[0] for row in rows] == [os.fsencode(root / 'a'), os.fsencode(root / 'b')]
        assert f['close'](stream) == 0
        if os.getpid() != parent_pid:
            os._exit(0)

print('FTS_TRAVERSAL_SKIP_FOLLOW_CYCLE_SORT_CHDIR_FORK_OK')
