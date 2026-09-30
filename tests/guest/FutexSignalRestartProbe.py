"""Verify old and modern futex signal restart rules with real guest callbacks."""
import ctypes as c
import errno
import json
import os
import threading
import time

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long] + [c.c_ulonglong] * 6


def call(number, *arguments):
    c.set_errno(0)
    result = libc.syscall(number, *arguments, *([0] * (6 - len(arguments))))
    return result, c.get_errno()


class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_longlong), ('nanoseconds', c.c_longlong)]


class Entry(c.Structure):
    _fields_ = [('value', c.c_ulonglong), ('address', c.c_ulonglong),
                ('flags', c.c_uint), ('reserved', c.c_uint)]


class Action(c.Structure):
    _fields_ = [('handler', c.c_ulonglong), ('flags', c.c_ulonglong),
                ('restorer', c.c_ulonglong), ('mask', c.c_ulonglong)]


observed = []
handler_wakes = []
active = None


@c.CFUNCTYPE(None, c.c_int)
def handler(number):
    handler_wakes.append(call(454, active[0], 0xffffffff, 1, active[1]))
    observed.append(number)


def queued(address, flags):
    entries = (Entry * 2)(Entry(0, address, flags, 0), Entry(0, address, flags, 0))
    end = time.monotonic() + 5
    while time.monotonic() < end:
        result = call(456, c.addressof(entries), 0, 0, 0x7fffffff)
        assert result in [(0, 0), (1, 0)], result
        if result[0] == 1:
            return
        time.sleep(.001)
    raise AssertionError('waiter did not enter queue')


def check(number, private, restart, timed=True, mutate=None):
    global active
    word = c.c_int()
    address = c.addressof(word)
    flags = 0x82 if private else 2
    active = address, flags
    action = Action(c.cast(handler, c.c_void_p).value, 0x10000000 if restart else 0, 0, 0)
    old = Action()
    assert call(13, 12, c.addressof(action), c.addressof(old), 8) == (0, 0)
    if number == 202:
        timeout = Timespec(5, 0)
    else:
        now = time.monotonic_ns() + 5_000_000_000
        timeout = Timespec(now // 1_000_000_000, now % 1_000_000_000)
    entries = (Entry * 1)(Entry(0, address, flags, 0))
    results, ids = [], []
    begin = len(observed)

    def waiting():
        ids.append(call(186)[0])
        if number == 202:
            results.append(call(202, address, 128 if private else 0, 0,
                                c.addressof(timeout) if timed else 0))
        elif number == 455:
            results.append(call(455, address, 0, 1, flags,
                                c.addressof(timeout) if timed else 0, 1 if timed else 999))
        else:
            results.append(call(449, c.addressof(entries), 1, 0,
                                c.addressof(timeout) if timed else 0, 1 if timed else 999))

    thread = threading.Thread(target=waiting)
    thread.start()
    queued(address, flags)
    if mutate == 'timeout':
        timeout.seconds = timeout.nanoseconds = 0
    if mutate == 'descriptor':
        entries[0].reserved = 1
    assert call(234, os.getpid(), ids[0], 12) == (0, 0)
    end = time.monotonic() + 5
    while len(observed) == begin and time.monotonic() < end:
        time.sleep(.001)
    assert len(observed) == begin + 1, observed
    assert handler_wakes[-1] == (0, 0), handler_wakes
    if restart and ((number == 202 and not timed) or (number != 202 and mutate is None)):
        queued(address, flags)
        assert call(454, address, 0xffffffff, 1, flags) == (1, 0)
        expected = (0, 0)
    elif number != 202 and restart:
        expected = (-1, errno.EINVAL if mutate == 'descriptor' else errno.ETIMEDOUT)
    else:
        expected = (-1, errno.EINTR)
    thread.join(timeout=6)
    assert not thread.is_alive() and results == [expected], (number, private, restart, timed, mutate, results)
    assert call(454, address, 0xffffffff, 1, flags) == (0, 0)
    assert call(13, 12, c.addressof(old), 0, 8) == (0, 0)


for private in [False, True]:
    for restart in [False, True]:
        for timed in [False, True]:
            check(202, private, restart, timed)
        for number in [455, 449]:
            for timed in [False, True]:
                check(number, private, restart, timed)
            if restart:
                check(number, private, restart, mutate='timeout')
                if number == 449:
                    check(number, private, restart, mutate='descriptor')

print('FUTEX_SIGNAL_RESTART_PASS ' + json.dumps(dict(legacy_timed=True,
    legacy_untimed=True, scalar=True, vector=True, argument_recopy=True,
    private=True, unflagged=True, retired_before_handler=True)), flush=True)
