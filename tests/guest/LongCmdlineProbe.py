"""Independent full /proc/cmdline regression, including arguments over 512 bytes."""
import os
import subprocess
import sys

payload = 'x' * 4096
script = ('import sys; arguments=open("/proc/self/cmdline","rb").read().split(b"\\0"); '
          'assert arguments[3]==sys.argv[1].encode(), (len(arguments[3]),len(sys.argv[1]))')
result = subprocess.run([sys.executable, '-c', script, payload], env={**os.environ, 'LANG': 'C.UTF-8'})
assert result.returncode == 0
print('LONG_PROC_CMDLINE_OK', flush=True)
