"""Long Linux argv survives fork/exec, spawn and subprocess transport."""
import hashlib
import os
import subprocess
import sys
import traceback


script = ('import hashlib,sys; '
          'assert hashlib.sha256(sys.argv[1].encode()).hexdigest()==sys.argv[2], "payload"; '
          'assert sys.argv[3]=="", "empty"; assert sys.argv[4]=="中文😀", "unicode"; '
          'recorded=open("/proc/self/cmdline","rb").read().split(b"\\0"); '
          'assert recorded[3]==sys.argv[1].encode(), "cmdline"; '
          'print("LARGE_EXEC_CHILD_OK")')
environment = {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'}
for payload in ('a' * 60000, 'q"\\\n' * 14000, '中文😀' * 6000):
    arguments = [sys.executable, '-c', script, payload,
                 hashlib.sha256(payload.encode()).hexdigest(), '', '中文😀']
    child = os.fork()
    if child == 0:
        try:
            os.execve(sys.executable, arguments, environment)
        except BaseException:
            traceback.print_exc()
            os._exit(1)
    assert os.waitpid(child, 0) == (child, 0)
    child = os.posix_spawn(sys.executable, arguments, environment)
    assert os.waitpid(child, 0) == (child, 0)
    result = subprocess.run(arguments, env=environment, text=True, capture_output=True, timeout=20)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == 'LARGE_EXEC_CHILD_OK', result.stdout

print('LARGE_EXEC_ARGUMENTS_OK', flush=True)
