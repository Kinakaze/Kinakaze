"""POSIX mask inheritance prevents worker threads stealing sigwait signals."""
import os
import signal
import threading

selected = {signal.SIGTERM, signal.SIGUSR1}
old = signal.pthread_sigmask(signal.SIG_BLOCK, selected)
observed = []
try:
    def worker():
        observed.append(signal.pthread_sigmask(signal.SIG_BLOCK, set()))
        signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGUSR1})
    for _ in range(8):
        thread = threading.Thread(target=worker)
        thread.start()
        thread.join(5)
        assert not thread.is_alive()
    assert all(selected <= mask for mask in observed), observed
    assert selected <= signal.pthread_sigmask(signal.SIG_BLOCK, set())
    received = []
    thread = threading.Thread(target=lambda: received.append(signal.sigwait(selected)))
    thread.start()
    os.kill(os.getpid(), signal.SIGTERM)
    thread.join(5)
    assert not thread.is_alive() and received == [signal.SIGTERM]
finally:
    signal.pthread_sigmask(signal.SIG_SETMASK, old)
print('PTHREAD_MASK_INHERIT_SIGWAIT_OK', flush=True)
