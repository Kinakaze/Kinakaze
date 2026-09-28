"""Redirected daemons must not retain abandoned bootstrap pipe endpoints."""
import errno
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile


with tempfile.TemporaryDirectory(prefix='standard-handles-') as directory:
    marker = Path(directory) / 'daemon'
    child = r'''
import os, sys, time
pid = os.fork()
if pid:
    os._exit(0)
os.setsid()
os.close(1)
# stdout and stderr initially share the native pipe handle. Closing stdout
# must leave stderr writable until its own last close.
os.write(2, b"STDERR_SURVIVED\n")
os.close(2)
fd = os.open('/dev/null', os.O_WRONLY)
os.dup2(fd, 1)
os.dup2(fd, 2)
if fd > 2:
    os.close(fd)
with open(sys.argv[1], 'w') as stream:
    stream.write(str(os.getpid()))
time.sleep(30)
'''
    process = subprocess.Popen([sys.executable, '-c', child, str(marker)],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        output, _ = process.communicate(timeout=5)
        assert process.returncode == 0, process.returncode
        assert output == b'STDERR_SURVIVED\n', output
        # The daemon can publish the marker just after closing the pipe.
        import time
        deadline = time.monotonic() + 2
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert marker.exists(), 'daemon did not redirect successfully'
        os.kill(int(marker.read_text()), 0)
    finally:
        if marker.exists():
            try:
                os.kill(int(marker.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass
        if process.poll() is None:
            process.kill()
        process.communicate(timeout=5)

check_closed = '''import errno,fcntl,os
for fd in (0,1,2):
    try: fcntl.fcntl(fd,fcntl.F_GETFD)
    except OSError as error:
        if error.errno != errno.EBADF: os._exit(20+fd)
    else: os._exit(30+fd)
os._exit(0)
'''
close_and_exec = '''import os,sys
for fd in (0,1,2): os.close(fd)
os.execv(sys.executable,[sys.executable,'-c',sys.argv[1]])
'''
result = subprocess.run([sys.executable, '-c', close_and_exec, check_closed],
    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=10)
assert result.returncode == 0, (result.returncode, result.stdout)
cloexec = '''import fcntl,os,sys
for fd in (0,1,2): fcntl.fcntl(fd,fcntl.F_SETFD,fcntl.FD_CLOEXEC)
os.execv(sys.executable,[sys.executable,'-c',sys.argv[1]])
'''
result = subprocess.run([sys.executable, '-c', cloexec, check_closed],
    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=10)
assert result.returncode == 0, (result.returncode, result.stdout)
print('STANDARD_HANDLE_LIFETIME_OK', flush=True)
