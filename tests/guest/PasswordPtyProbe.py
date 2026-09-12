"""Read getpass through a controlling PTY and verify echo suppression/restoration."""
import errno
import fcntl
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import termios
import time

source = r'''
#define _GNU_SOURCE
#include <assert.h>
#include <string.h>
#include <stdio.h>
#include <termios.h>
#include <unistd.h>
int main(void){
    struct termios before,after;assert(!tcgetattr(0,&before));
    char *password=getpass("Password: ");assert(password && !strcmp(password,"private-value"));
    assert(!tcgetattr(0,&after));
    assert(before.c_iflag==after.c_iflag && before.c_oflag==after.c_oflag && before.c_lflag==after.c_lflag);
    assert(!memcmp(before.c_cc,after.c_cc,sizeof before.c_cc));
    puts("PASSWORD_PTY_OK");return 0;
}
'''
with tempfile.TemporaryDirectory(prefix='kinakaze-password-') as directory:
    program = str(Path(directory) / 'probe')
    subprocess.run(['/usr/bin/gcc', '-x', 'c', '-o', program, '-'], input=source.encode(), check=True, timeout=30)
    master, slave = os.openpty()
    child = os.fork()
    if child == 0:
        os.close(master)
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        for fd in range(3): os.dup2(slave, fd)
        if slave > 2: os.close(slave)
        os.execv(program, [program])
    output = b''
    finished = False
    try:
        deadline = time.monotonic() + 15
        sent = False
        while b'PASSWORD_PTY_OK' not in output:
            assert time.monotonic() < deadline, output
            ready, _, _ = select.select([master], [], [], 0.1)
            if not ready: continue
            try: chunk = os.read(master, 4096)
            except OSError as error:
                if error.errno == errno.EIO: break
                raise
            if not chunk: break
            output += chunk
            if b'Password: ' in output and not sent:
                assert not termios.tcgetattr(slave)[3] & (termios.ECHO | termios.ISIG)
                os.write(master, b'private-value\n')
                sent = True
        assert b'PASSWORD_PTY_OK' in output and b'private-value' not in output, output
        while True:
            pid, status = os.waitpid(child, os.WNOHANG)
            if pid:
                finished = True
                assert status == 0, (status, output)
                break
            assert time.monotonic() < deadline
            time.sleep(0.01)
    finally:
        if not finished:
            os.kill(child, signal.SIGKILL)
            os.waitpid(child, 0)
        os.close(master)
        os.close(slave)
print('PASSWORD_TERMINAL_RESTORED_OK')
