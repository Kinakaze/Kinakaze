"""Cross-process priority changes must affect the target, preserve the caller,
and reject a PID once the target has exited and been reaped.
"""

import errno
import os
import subprocess
import sys


def main():
    original = os.getpriority(os.PRIO_PROCESS, 0)
    child = subprocess.Popen(
        [
            sys.executable,
            "-u",
            "-c",
            "import os,sys; print(os.getpid(), flush=True); "
            "sys.stdin.readline(); print(os.getpriority(os.PRIO_PROCESS,0),flush=True); "
            "sys.stdin.readline()",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        pid = int(child.stdout.readline())
        assert pid == child.pid, (pid, child.pid)
        assert os.getpriority(os.PRIO_PROCESS, pid) == original
        os.setpriority(os.PRIO_PROCESS, pid, 10)
        assert os.getpriority(os.PRIO_PROCESS, 0) == original
        target_value = os.getpriority(os.PRIO_PROCESS, pid)
        assert target_value >= 10, target_value
        child.stdin.write("query\n")
        child.stdin.flush()
        assert int(child.stdout.readline()) == target_value
        child.stdin.write("exit\n")
        child.stdin.flush()
        assert child.wait(timeout=10) == 0
        for operation in (
            lambda: os.getpriority(os.PRIO_PROCESS, pid),
            lambda: os.setpriority(os.PRIO_PROCESS, pid, 10),
        ):
            try:
                operation()
            except OSError as error:
                assert error.errno == errno.ESRCH, error
            else:
                raise AssertionError("reaped PID accepted")
        observer = subprocess.run(
            [
                sys.executable,
                "-c",
                "import errno,os,sys; os.seteuid(65534)\n"
                "try: os.setpriority(os.PRIO_PROCESS,int(sys.argv[1]),10)\n"
                "except OSError as error: assert error.errno==errno.EPERM,error\n"
                'else: raise AssertionError("foreign UID accepted")',
                str(os.getpid()),
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert observer.returncode == 0, (observer.stdout, observer.stderr)
        assert os.getpriority(os.PRIO_PROCESS, 0) == original
        print("PROCESS_PRIORITY_OK")
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=10)


if __name__ == "__main__":
    main()
