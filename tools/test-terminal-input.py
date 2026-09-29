"""Drive the native terminal client's raw console reader through a private ConPTY."""
import argparse
import json
import os
from pathlib import Path
import sys
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--test-exe', type=Path, required=True)
parser.add_argument('--python-path', type=Path)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
if args.python_path:
    sys.path.insert(0, str(args.python_path))
from winpty import PtyProcess

environment = dict(os.environ)
environment['PATH'] = str(args.test_exe.resolve().parent.parent) + os.pathsep + environment.get('PATH', '')
process = PtyProcess.spawn([str(args.test_exe.resolve()),
    'console_tests::ctrl_c_bytes_reach_guest_transport', '--ignored', '--nocapture'],
    dimensions=(30, 120), env=environment)
output = []
def read():
    try:
        while process.isalive():
            output.append(process.read(4096))
    except (EOFError, OSError):
        pass
thread = threading.Thread(target=read, daemon=True)
thread.start()
passed = False
try:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and 'CONSOLE_INPUT_READY' not in ''.join(output):
        time.sleep(.05)
    assert 'CONSOLE_INPUT_READY' in ''.join(output), ''.join(output)
    process.write('\x03')
    time.sleep(.3)
    process.write('\x03')
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if 'CONSOLE_CTRL_C_OK' in ''.join(output):
            passed = True
            break
        time.sleep(.05)
finally:
    process.close(force=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(dict(passed=passed, transcript=''.join(output)), indent=2), encoding='utf-8')
print(json.dumps(dict(passed=passed)))
raise SystemExit(0 if passed else 1)
