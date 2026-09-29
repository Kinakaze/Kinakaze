#!/usr/bin/python3
"""Distribution-provided guest session service; owns PTYs and reaps children.

Runs inside the downloaded Debian image, never in a host Python installation.
Only the native init owns the authenticated control listener. Client detach does
not close a PTY, send a signal, fork, or change a guest's parent.
"""
import ctypes
import errno
import fcntl
import json
import os
import select
import signal
import socket
import struct
import sys
import termios
import time

LIMIT = 1024 * 1024


def main():
    with open(sys.argv[1]) as source:
        config = json.load(source)
    os.environ.update(config['startup'].get('environment', {}))
    # Adopt double-forked descendants; Linux's live process registry owns PPID.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), 'PR_SET_CHILD_SUBREAPER')
    control = socket.create_connection(('127.0.0.1', config['port']), timeout=30)
    control.settimeout(None)
    control.set_inheritable(False)

    def send(value):
        control.sendall(json.dumps(value, separators=(',', ':')).encode() + b'\n')

    send({'role': 'broker', 'token': config['token']})
    poller = select.poll()
    poller.register(control, select.POLLIN)
    # Stream sockets have a native readiness notification in the hosted VFS.
    # A pipe in this mixed poll set would force a periodic readiness sweep.
    wake_reader, wake_writer = socket.socketpair()
    for wake_socket in (wake_reader, wake_writer):
        wake_socket.setblocking(False)
        wake_socket.set_inheritable(False)
    wake_r, wake_w = wake_reader.fileno(), wake_writer.fileno()
    poller.register(wake_r, select.POLLIN)
    signal.set_wakeup_fd(wake_w)
    stopping = False

    def terminate(*_):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGHUP, terminate)
    signal.signal(signal.SIGCHLD, lambda *_: None)
    terminals = {}
    masters = {}
    specs = {item['name']: item for item in config['startup']['terminals']}

    def spawn(spec):
        name = spec['name']
        previous = terminals.get(name)
        if previous is not None:
            if previous['spec'] != spec:
                raise ValueError('terminal name already belongs to another command; use a new name')
            return {k: v for k, v in previous.items() if k not in ('spec', 'pending')}
        if len(terminals) >= 128:
            raise ValueError('terminal limit reached')
        master, slave = os.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
        pid = os.fork()
        if pid == 0:
            try:
                signal.set_wakeup_fd(-1)
                for sig in (signal.SIGCHLD, signal.SIGTERM, signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTSTP, signal.SIGTTIN, signal.SIGTTOU, signal.SIGPIPE):
                    signal.signal(sig, signal.SIG_DFL)
                os.setsid()
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
                os.tcsetpgrp(slave, os.getpgrp())
                for fd in (0, 1, 2):
                    os.dup2(slave, fd)
                control.close()
                wake_reader.close()
                wake_writer.close()
                for fd in list(masters) + [master, slave]:
                    if fd > 2:
                        os.close(fd)
                os.chdir(spec.get('cwd', '/'))
                env = dict(os.environ)
                env.setdefault('TERM', 'xterm-256color')
                env.update(spec.get('environment', {}))
                os.execve(spec['command'][0], spec['command'], env)
            except BaseException as error:
                os.write(2, ('kinakaze: ' + str(error) + '\r\n').encode())
                os._exit(127)
        os.close(slave)
        os.set_blocking(master, False)
        terminals[name] = {'name': name, 'pid': pid, 'ppid': os.getpid(), 'status': None, 'spec': spec, 'pending': bytearray()}
        masters[master] = name
        poller.register(master, select.POLLIN)
        return {'name': name, 'pid': pid, 'ppid': os.getpid(), 'status': None}

    def reap():
        while True:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return
            if not pid:
                return
            for item in terminals.values():
                if item['pid'] == pid:
                    item['status'] = os.waitstatus_to_exitcode(status)
                    # Keep output readable until the PTY closes. Exit is a state
                    # event, not permission for the host to consume guest wait().
                    send({'event': 'exit', 'name': item['name'], 'status': item['status']})
                    break

    def resize(name, rows, cols):
        fd = next(fd for fd, value in masters.items() if value == name)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', max(1, min(rows, 500)), max(1, min(cols, 500)), 0, 0))

    def command(request):
        nonlocal stopping
        op = request['op']
        if op == 'start':
            spec = request.get('terminal') or specs[request['name']]
            if not spec['command'][0].startswith('/') or not spec.get('cwd', '/root').startswith('/'):
                raise ValueError('absolute executable and cwd required')
            result = spawn(spec)
            send({'event': 'terminal', **result})
            return result
        if op == 'input':
            name = request['name']
            data = bytes.fromhex(request['data'])
            item = terminals[name]
            if len(item['pending']) + len(data) > LIMIT:
                raise ValueError('terminal input buffer full')
            item['pending'].extend(data)
            fd = next(fd for fd, value in masters.items() if value == name)
            poller.modify(fd, select.POLLIN | select.POLLOUT)
        elif op == 'resize':
            resize(request['name'], int(request['rows']), int(request['cols']))
        elif op == 'signal':
            pid = int(request['pid'])
            if pid <= 1 or pid == os.getpid():
                raise ValueError('use shutdown to stop the process tree')
            os.kill(pid, signal.SIGTERM)
        elif op == 'processes':
            result = []
            for entry in os.listdir('/proc'):
                if not entry.isdigit():
                    continue
                try:
                    with open('/proc/' + entry + '/stat') as source:
                        stat = source.read().rsplit(')', 1)[1].split()
                    with open('/proc/' + entry + '/comm') as source:
                        name = source.read().strip()
                    result.append({'pid': int(entry), 'ppid': int(stat[1]), 'program': name})
                except (OSError, ValueError, IndexError):
                    pass
            return result
        elif op == 'shutdown':
            shutdown = config['startup'].get('shutdown', [])
            if shutdown:
                pid = os.fork()
                if pid == 0:
                    os.execve(shutdown[0], shutdown, os.environ)
                    os._exit(127)
            elif os.getpid() == 1:
                stopping = True
            else:
                os.kill(1, signal.SIGTERM)
        else:
            raise ValueError('unknown operation')
        return {}

    try:
        script = config['startup'].get('script')
        if script:
            pid = os.fork()
            if pid == 0:
                os.execve(script[0], script, os.environ)
                os._exit(127)
            _, status = os.waitpid(pid, 0)
            if status:
                raise RuntimeError('startup script failed: ' + str(os.waitstatus_to_exitcode(status)))
        for spec in specs.values():
            if spec.get('autostart', True):
                send({'event': 'terminal', **spawn(spec)})
        send({'event': 'ready', 'pid': os.getpid(), 'ppid': os.getppid()})
        incoming = bytearray()
        while not stopping:
            for fd, events in poller.poll():
                if fd == wake_r:
                    try:
                        os.read(wake_r, 4096)
                    except BlockingIOError:
                        pass
                    reap()
                elif fd == control.fileno():
                    data = control.recv(65536)
                    if not data:
                        stopping = True
                        break
                    incoming.extend(data)
                    if len(incoming) > LIMIT:
                        raise ValueError('oversized control frame')
                    while b'\n' in incoming:
                        line, _, incoming = incoming.partition(b'\n')
                        request = json.loads(line)
                        try:
                            result = command(request)
                            send({'id': request['id'], 'ok': result})
                        except (OSError, ValueError, KeyError, StopIteration) as error:
                            send({'id': request['id'], 'error': str(error)})
                elif fd in masters:
                    name = masters[fd]
                    item = terminals[name]
                    if events & select.POLLOUT and item['pending']:
                        try:
                            count = os.write(fd, item['pending'])
                            del item['pending'][:count]
                        except BlockingIOError:
                            pass
                        if not item['pending']:
                            poller.modify(fd, select.POLLIN)
                    if events & (select.POLLIN | select.POLLHUP | select.POLLERR):
                        try:
                            data = os.read(fd, 8192)
                        except BlockingIOError:
                            continue
                        except OSError as error:
                            if error.errno != errno.EIO:
                                raise
                            data = b''
                        if data:
                            send({'event': 'output', 'name': name, 'data': data.hex()})
                        else:
                            poller.unregister(fd)
                            os.close(fd)
                            del masters[fd]
                            reap()
                            send({'event': 'closed', 'name': name})
            reap()
    finally:
        signal.set_wakeup_fd(-1)
        for fd in masters:
            os.close(fd)
        # Includes adopted/double-forked descendants, not merely the original
        # process group. No process belonging to another session is signaled.
        def descendants():
            parents = {}
            for entry in os.listdir('/proc'):
                if entry.isdigit():
                    try:
                        with open('/proc/' + entry + '/stat') as source:
                            fields = source.read().rsplit(')', 1)[1].split()
                        parents[int(entry)] = int(fields[1])
                    except (OSError, ValueError, IndexError):
                        pass
            owned = {os.getpid()}
            while True:
                extended = owned | {pid for pid, parent in parents.items() if parent in owned}
                if extended == owned:
                    return owned - {os.getpid()}
                owned = extended
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for pid in descendants():
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
            deadline = time.monotonic() + 1
            while time.monotonic() < deadline:
                try:
                    pid, _ = os.waitpid(-1, os.WNOHANG)
                    if not pid:
                        time.sleep(.02)
                except ChildProcessError:
                    break
        control.close()


if __name__ == '__main__':
    main()
