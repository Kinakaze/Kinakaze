"""Shared mutex exclusion, ownership, deadlines, aliases and owner recovery."""
import ctypes as C
import os
from pathlib import Path

C.CDLL('libpthread.so.0', mode=C.RTLD_GLOBAL)
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
probe.probe_shared_types.argtypes = [C.c_int, C.c_int]
probe.probe_shared_timed.argtypes = [C.c_int]
probe.probe_shared_death.argtypes = [C.c_int] * 4
probe.probe_shared_alias.argtypes = [C.c_char_p]
for kind in range(4):
    for robust in (0, 1):
        result = probe.probe_shared_types(kind, robust)
        assert result == 0, ('types', kind, robust, result)
for clock in (0, 1):
    result = probe.probe_shared_timed(clock)
    assert result == 0, ('timed', clock, result)
for kind in range(4):
    for mode in range(5):
        for poison in (0, 1):
            result = probe.probe_shared_death(kind, mode, poison, 1)
            assert result == 0, ('death', kind, mode, poison, result)
    result = probe.probe_shared_death(kind, 4, 0, 0)
    assert result == 0, ('stalled', kind, result)
result = probe.probe_shared_alias(f'/tmp/pthread-mutex-alias-{os.getpid()}'.encode())
assert result == 0, ('alias', result)
loader = C.CDLL('libdl.so.2')
loader.dlvsym.argtypes = [C.c_void_p, C.c_char_p, C.c_char_p]
loader.dlvsym.restype = C.c_void_p
for soname, versions in (('libpthread.so.0', ('GLIBC_2.2.5',)),
                         ('libc.so.6', ('GLIBC_2.2.5', 'GLIBC_2.34'))):
    library = C.CDLL(soname)
    for name in ('pthread_mutexattr_getpshared', 'pthread_mutexattr_setpshared'):
        expected = C.cast(getattr(library, name), C.c_void_p).value
        for version in versions:
            assert loader.dlvsym(library._handle, name.encode(), version.encode()) == expected
        assert loader.dlvsym(library._handle, name.encode(), b'GLIBC_999.0') is None
print('PTHREAD_SHARED_MUTEX_OK', flush=True)
