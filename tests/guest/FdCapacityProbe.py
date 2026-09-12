"""Sparse high fd numbers, enforced limits, readiness, fork/exec and close_range."""
import array
import ctypes as c
import errno
import fcntl
import os
from pathlib import Path
import resource
import select
import socket
import sys
import tempfile
import time

lib=c.CDLL('libc.so.6',use_errno=True)
lib.close_range.argtypes=[c.c_uint,c.c_uint,c.c_uint]
lib.select.argtypes=[c.c_int,c.c_void_p,c.c_void_p,c.c_void_p,c.c_void_p]
lib.setrlimit.argtypes=[c.c_int,c.c_void_p]
class Timeval(c.Structure):
    _fields_=[('seconds',c.c_long),('microseconds',c.c_long)]

def fails(code, operation, *args):
    try: operation(*args)
    except OSError as error: assert error.errno==code,(error,code)
    else: raise AssertionError(('unexpected success',operation,args))

def wait(child):
    assert os.waitpid(child,0)==(child,0)

def setlimit(soft,hard):
    values=(c.c_ulonglong*2)(soft,hard)
    if lib.setrlimit(resource.RLIMIT_NOFILE,values):
        raise OSError(c.get_errno(),'setrlimit')

def proc_limit(pid='self'):
    row=next(row for row in Path('/proc/'+str(pid)+'/limits').read_text().splitlines()
             if row.startswith('Max open files'))
    return tuple(map(int,row.split()[3:5]))

limit=resource.RLIMIT_NOFILE
original=resource.getrlimit(limit)
assert proc_limit()==original
ceiling=int(Path('/proc/sys/fs/nr_open').read_text())
assert ceiling>=65536
resource.setrlimit(limit,(65536,ceiling))
assert resource.getrlimit(limit)==(65536,ceiling)
assert proc_limit()==(65536,ceiling)
# Empty select is a timed wait and must not return immediately.
tv=Timeval(0,30000); started=time.monotonic()
assert lib.select(0,None,None,None,c.byref(tv))==0
assert time.monotonic()-started>=0.025 and (tv.seconds,tv.microseconds)==(0,0)
with tempfile.TemporaryFile() as file:
    file.write(b'0123456789abcdef');file.flush();file.seek(0)
    high=65535
    assert os.dup2(file.fileno(),high,inheritable=True)==high
    assert os.read(high,2)==b'01' and file.tell()==2
    fails(errno.EBADF,os.dup2,file.fileno(),65536)
    resource.setrlimit(limit,(ceiling,ceiling))
    highest=ceiling-1
    if highest!=high: os.dup2(high,highest,inheritable=True);os.close(high)
    high=highest
    assert os.fstat(high).st_ino==os.fstat(file.fileno()).st_ino
    assert str(high) in os.listdir('/proc/self/fd')
    assert os.readlink('/proc/self/fd/'+str(high))
    polls=select.poll();polls.register(high,select.POLLIN)
    assert polls.poll(0)==[(high,select.POLLIN)]
    words=(high+64)//64
    reads=(c.c_ulong*words)();writes=(c.c_ulong*words)()
    reads[high//64]=writes[high//64]=1<<(high%64)
    tv=Timeval()
    assert lib.select(high+1,reads,writes,None,c.byref(tv))==2,c.get_errno()
    assert reads[high//64]==writes[high//64]==1<<(high%64)
    # A small bitmap must not be overwritten as a full 1024-bit C fd_set.
    low=file.fileno();assert low<64
    small=(c.c_ulong*2)(1<<low,0xdeadbeefcafef00d)
    assert lib.select(low+1,small,None,None,c.byref(tv))==1
    assert small[1]==0xdeadbeefcafef00d
    bad=(c.c_ulong*2)(1<<63,0x1234)
    assert lib.select(64,bad,None,None,c.byref(tv))==-1 and c.get_errno()==errno.EBADF
    assert bad[1]==0x1234
    child=os.fork()
    if child==0:
        try:
            assert os.read(high,2)==b'23'
            os.execl('/usr/bin/python3.11','python3.11','-c',
                     'import os,sys;assert os.read(int(sys.argv[1]),2)==b"45";print("HIGH_FD_EXEC_OK")',str(high))
        except BaseException: os._exit(71)
    wait(child);assert file.tell()==6
    sender,receiver=socket.socketpair()
    sender.sendmsg([b'F'],[(socket.SOL_SOCKET,socket.SCM_RIGHTS,array.array('i',[high]))])
    data,controls,flags,_=receiver.recvmsg(1,socket.CMSG_SPACE(4),socket.MSG_CMSG_CLOEXEC)
    assert data==b'F' and not flags&socket.MSG_CTRUNC
    transferred=array.array('i');transferred.frombytes(controls[0][2]);fd=transferred[0]
    assert os.read(fd,2)==b'67' and not os.get_inheritable(fd)
    os.close(fd);sender.close();receiver.close()
    descriptors=[os.dup(file.fileno()) for _ in range(2048)]
    assert max(descriptors)>1024
    child=os.fork()
    if child==0:
        try:
            assert len(os.listdir('/proc/self/fd'))>=2048
            for fd in descriptors[::127]:
                assert os.fstat(fd).st_ino==os.fstat(file.fileno()).st_ino
            os._exit(0)
        except BaseException: os._exit(73)
    wait(child)
    boundary=max(descriptors)+1
    resource.setrlimit(limit,(boundary,ceiling))
    fails(errno.EMFILE,os.dup,file.fileno())
    for hole in (descriptors[10],descriptors[1000]):
        os.close(hole);assert os.dup(file.fileno())==hole
    for fd in descriptors: os.close(fd)
    assert proc_limit()==(boundary,ceiling)
    resource.setrlimit(limit,(ceiling,ceiling))
    assert lib.close_range(high,high,1)==-1 and c.get_errno()==errno.EINVAL
    assert fcntl.fcntl(high,fcntl.F_GETFD)==0
    assert lib.close_range(high,high,4)==0
    assert fcntl.fcntl(high,fcntl.F_GETFD)&fcntl.FD_CLOEXEC
    child=os.fork()
    if child==0:
        code='import os,sys,errno\ntry:os.fstat(int(sys.argv[1]))\nexcept OSError as e:assert e.errno==errno.EBADF\nelse:raise AssertionError("CLOEXEC fd survived")'
        os.execl('/usr/bin/python3.11','python3.11','-c',code,str(high))
    wait(child)
    assert lib.close_range(high,0xffffffff,0)==0
    fails(errno.EBADF,os.fstat,high)

# High numbered eventfd readiness must use the same open-description counter.
event=os.eventfd(0,os.EFD_NONBLOCK|os.EFD_CLOEXEC)
os.dup2(event,ceiling-1,inheritable=False)
with select.epoll() as watcher:
    watcher.register(ceiling-1,select.EPOLLIN)
    assert watcher.poll(0)==[]
    os.eventfd_write(event,7)
    assert watcher.poll(0)==[(ceiling-1,select.EPOLLIN)]
    assert os.eventfd_read(ceiling-1)==7
os.close(event);os.close(ceiling-1)

fails(errno.EPERM,setlimit,ceiling+1,ceiling+1)
child=os.fork()
if child==0:
    try:
        os.setresuid(65534,65534,65534)
        resource.setrlimit(limit,(128,256))
        resource.setrlimit(limit,(256,256))
        fails(errno.EPERM,setlimit,512,512)
        os._exit(0)
    except BaseException: os._exit(72)
wait(child)
resource.setrlimit(limit,original)
assert proc_limit()==original
print('FD_CAPACITY_SPARSE_LIMITS_SELECT_RIGHTS_FORK_EXEC_OK')
