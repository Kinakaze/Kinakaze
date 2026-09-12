"""A failed graph/relocation must not contaminate later dlopen or finalizers."""
import ctypes as c
import os
import sys

directory = sys.argv[1]
for name in ('bad-symbol.so', 'bad-needed.so'):
    for _ in range(3):
        try:
            c.CDLL(directory + '/' + name)
        except OSError:
            pass
        else:
            raise AssertionError('Unresolved library unexpectedly loaded: ' + name)
        try:
            c.CDLL(directory + '/' + name, mode=os.RTLD_NOW | os.RTLD_NOLOAD)
        except OSError:
            pass
        else:
            raise AssertionError('Failed library leaked into loader scope: ' + name)
        good = c.CDLL(directory + '/good.so')
        assert good.probe_value() == 42
print('DLOPEN_FAILURE_RECOVERY_OK', flush=True)
