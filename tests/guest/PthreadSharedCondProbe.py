"""Cross-process shared condition handoff, aliases, broadcast and repair."""
import ctypes as C
import os
from pathlib import Path

C.CDLL('libpthread.so.0', mode=C.RTLD_GLOBAL)
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
probe.probe_shared_cond_handoff.argtypes = [C.c_int, C.c_int, C.c_char_p]
probe.probe_shared_cond_handoff.restype = C.c_longlong
probe.probe_shared_cond_broadcast.argtypes = [C.c_int, C.c_int]
probe.probe_shared_cond_owner_death.argtypes = [C.c_int]
for clock in (0, 1):
    for alias in (False, True):
        path = f'/tmp/shared-cond-alias-{os.getpid()}-{clock}'.encode() if alias else None
        result = probe.probe_shared_cond_handoff(clock, 200, path)
        assert result > 0, ('handoff', clock, alias, result)
    result = probe.probe_shared_cond_broadcast(clock, 4)
    assert result == 0, ('broadcast', clock, result)
    result = probe.probe_shared_cond_owner_death(clock)
    assert result == 0, ('death', clock, result)
loader = C.CDLL('libdl.so.2')
loader.dlvsym.argtypes = [C.c_void_p, C.c_char_p, C.c_char_p]
loader.dlvsym.restype = C.c_void_p
for soname, versions in (('libpthread.so.0', ('GLIBC_2.2.5',)),
                         ('libc.so.6', ('GLIBC_2.2.5', 'GLIBC_2.34'))):
    library = C.CDLL(soname)
    for name in ('pthread_condattr_getpshared', 'pthread_condattr_setpshared'):
        expected = C.cast(getattr(library, name), C.c_void_p).value
        for version in versions:
            assert loader.dlvsym(library._handle, name.encode(), version.encode()) == expected
        assert loader.dlvsym(library._handle, name.encode(), b'GLIBC_999.0') is None
print('PTHREAD_SHARED_COND_OK', flush=True)
