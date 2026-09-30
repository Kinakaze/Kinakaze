"""Native timed handoff, signal delivery, and condition clocks across fork."""
import ctypes as C
from pathlib import Path
import sys

C.CDLL('libpthread.so.0', mode=C.RTLD_GLOBAL)
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
cases = {'timed': 'probe_timed_mutex_handoff', 'signal': 'probe_cond_signal',
         'fork': 'probe_cond_fork_clock'}
for case in sys.argv[1:] or cases:
    result = getattr(probe, cases[case])()
    assert result == 0, (case, result)
    print('PTHREAD_EVENT_' + case.upper() + '_OK', flush=True)
