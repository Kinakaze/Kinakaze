"""Repeated exec keeps Linux identity and live IPC while native workers change."""
import errno
import json
import os
import shutil
import signal
import socket
import sys
import tempfile

ROUNDS = 24

if len(sys.argv) > 1 and sys.argv[1] == "cleanup":
    state = json.loads(os.environ["EXEC_POOL_STATE"])
    assert os.getpid() == state["pid"]
    shutil.rmtree(state["directory"])
    print("EXEC_PREWARM_POOL_OK", flush=True)
    sys.exit(0)

if len(sys.argv) == 1:
    directory = tempfile.mkdtemp(prefix="exec-prewarm-")
    os.mkdir(directory + "/a")
    os.mkdir(directory + "/b")
    data = os.open(directory + "/data", os.O_CREAT | os.O_RDWR, 0o600)
    os.write(data, b"abcdefghijklmnopqrstuvwxyz")
    os.lseek(data, 0, os.SEEK_SET)
    output = os.open(directory + "/output", os.O_CREAT | os.O_RDWR, 0o600)
    saved_stdout = os.dup(1)
    pipe = os.pipe()
    unix = socket.socketpair()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(("127.0.0.1", 0))
    event = os.eventfd(1, os.EFD_NONBLOCK)
    closed = os.open(directory + "/data", os.O_RDONLY | os.O_CLOEXEC)
    fds = [data, output, saved_stdout, *pipe, *(s.fileno() for s in unix), udp.fileno(), event]
    for fd in fds:
        os.set_inheritable(fd, True)
    state = dict(directory=directory, fds=fds, closed=closed, pid=os.getpid(),
                 port=udp.getsockname()[1], script=os.path.abspath(__file__))
    os.dup2(output, 1)
    signal.signal(signal.SIGUSR1, signal.SIG_IGN)
    signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR2})
    round_number = 0
else:
    state = json.loads(os.environ["EXEC_POOL_STATE"])
    round_number = int(sys.argv[1])
    data, output, saved_stdout, read, write, left, right, network, event = state["fds"]
    pipe = (read, write)
    unix = (socket.socket(fileno=left), socket.socket(fileno=right))
    udp = socket.socket(fileno=network)
    try:
        os.fstat(state["closed"])
    except OSError as error:
        assert error.errno == errno.EBADF
    else:
        raise AssertionError("CLOEXEC descriptor survived")
    assert os.getcwd() == state["directory"] + ("/a" if round_number % 2 else "/b")
    assert os.environ["EXEC_POOL_ROUND"] == str(round_number)
    assert signal.getsignal(signal.SIGALRM) == signal.SIG_DFL

assert os.getpid() == state["pid"]
assert signal.getsignal(signal.SIGUSR1) == signal.SIG_IGN
assert signal.SIGUSR2 in signal.pthread_sigmask(signal.SIG_BLOCK, set())
assert os.read(data, 1) == bytes([ord("a") + round_number])
assert os.lseek(data, 0, os.SEEK_CUR) == round_number + 1
os.write(1, b"x")
os.write(pipe[1], b"p")
assert os.read(pipe[0], 1) == b"p"
unix[0].sendall(b"u")
assert unix[1].recv(1) == b"u"
assert udp.getsockname()[1] == state["port"]
udp.sendto(b"d", ("127.0.0.1", state["port"]))
assert udp.recv(1) == b"d"
assert os.eventfd_read(event) == round_number + 1
os.eventfd_write(event, round_number + 2)

if round_number < ROUNDS:
    # A precommit failure leaves this image and its descriptors usable.
    try:
        os.execve("/missing-exec-prewarm-probe", ["missing"], {})
    except OSError as error:
        assert error.errno == errno.ENOENT
    next_round = round_number + 1
    os.chdir(state["directory"] + ("/a" if next_round % 2 else "/b"))
    signal.signal(signal.SIGALRM, lambda *_: None)
    environment = dict(os.environ, EXEC_POOL_STATE=json.dumps(state), EXEC_POOL_ROUND=str(next_round))
    os.execve(sys.executable, [sys.executable, state["script"], str(next_round)], environment)

os.dup2(saved_stdout, 1)
os.lseek(output, 0, os.SEEK_SET)
assert os.read(output, ROUNDS + 2) == b"x" * (ROUNDS + 1)
unix[0].close()
unix[1].close()
udp.close()
for fd in [data, output, saved_stdout, *pipe, event]:
    os.close(fd)
os.chdir("/")
# The last native wrapper still has a Windows cwd handle to directory/b.
# Replacing it from / releases that handle before this fixture removes its tree.
os.execve(sys.executable, [sys.executable, state["script"], "cleanup"], os.environ)
