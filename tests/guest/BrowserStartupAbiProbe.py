"""Browser import behavior, public buffer ownership and x87 caller ABI."""
import ctypes as C
import errno
import os
import select
import subprocess
import threading
import time
from pathlib import Path

subprocess.run([str(Path(__file__).with_name('BrowserForkTlsProbe'))],check=True,timeout=20)
subprocess.run(['/usr/bin/python3.11',str(Path(__file__).with_name('BrowserXcbSetupProbe.py'))],check=True,timeout=20)

P,I=C.c_void_p,C.c_int
libc=C.CDLL('libc.so.6',use_errno=True)
# Chromium's protected globals probe getrlimit64 with inaccessible destinations.
# Syscall output copies must report EFAULT without changing page protections.
libc.mmap.argtypes=[P,C.c_size_t,I,I,I,C.c_longlong]
libc.mmap.restype=P
libc.mprotect.argtypes=[P,C.c_size_t,I]
libc.munmap.argtypes=[P,C.c_size_t]
# V8 reserves tiny code ranges at randomized page hints that need not be
# Windows allocation-granularity aligned. Exercise reservation -> RW -> RX,
# actual machine-code execution, and reuse at the same page-aligned address.
for hint in (0x35000000f000,0x35000001ffff):
    code=libc.mmap(hint,8192,0,0x22,-1,0)
    assert code not in (None,C.c_void_p(-1).value)
    try:
        assert libc.mprotect(code,8192,3)==0, ('hinted code commit',hex(code),C.get_errno())
        C.memmove(code,b'\xb8\x2a\x00\x00\x00\xc3',6)
        assert libc.mprotect(code,8192,5)==0
        assert C.CFUNCTYPE(I)(code)()==42
        child=os.fork()
        if child==0:os._exit(0 if C.CFUNCTYPE(I)(code)()==42 else 31)
        assert os.waitpid(child,0)[1]==0, 'hinted executable mapping lost across fork'
    finally:assert libc.munmap(code,8192)==0
    exact=libc.mmap(hint&~4095,8192,0,0x32,-1,0)
    assert exact==hint&~4095, ('page-aligned MAP_FIXED reservation',hex(exact or 0))
    try:
        assert libc.mprotect(exact,8192,7)==0
        C.memmove(exact,b'\xb8\x2b\x00\x00\x00\xc3',6)
        assert C.CFUNCTYPE(I)(exact)()==43
    finally:assert libc.munmap(exact,8192)==0
# PartitionAlloc recycles a superpage spanning guard reservations and views.
span=2*1024*1024
region=libc.mmap(None,span+8192,0,0x22,-1,0)
assert region not in (None,C.c_void_p(-1).value)
try:
    for offset,size in ((0,4096),(8192,4096),(65536,131072),(span+4096,4096)):
        assert libc.mmap(region+offset,size,3,0x32,-1,0)==region+offset
        C.memset(region+offset,0x5a,size)
    recycled=libc.mmap(region+4096,span,0,0x32,-1,0)
    assert recycled==region+4096, ('fragmented MAP_FIXED PROT_NONE',C.get_errno())
    assert C.string_at(region,4096)==b'Z'*4096
    assert C.string_at(region+span+4096,4096)==b'Z'*4096
    for offset,size in ((8192,4096),(65536,131072)):
        assert libc.mprotect(region+offset,size,3)==0
        assert C.string_at(region+offset,size)==b'\0'*size
finally:assert libc.munmap(region,span+8192)==0
# Mio's eventfd waker deliberately writes repeatedly without draining.
fd=os.eventfd(0,os.EFD_NONBLOCK)
alias=os.dup(fd)
try:
    with select.epoll() as poller:
        poller.register(fd,select.EPOLLIN|select.EPOLLET)
        for _ in range(3):
            os.eventfd_write(alias,1)
            assert poller.poll(0.2)==[(fd,select.EPOLLIN)], 'lost undrained eventfd wake'
            assert poller.poll(0)==[]
        writer=threading.Thread(target=lambda:(time.sleep(0.05),os.eventfd_write(alias,1)))
        writer.start()
        assert poller.poll(1)==[(fd,select.EPOLLIN)], 'lost threaded eventfd wake'
        writer.join()
        child=os.fork()
        if child==0:
            if poller.poll(0)!=[]:os._exit(23)
            os.eventfd_write(alias,1)
            os._exit(0)
        assert poller.poll(2)==[(fd,select.EPOLLIN)], 'lost cross-process eventfd wake'
        assert os.waitpid(child,0)[1]==0
        assert os.eventfd_read(fd)==5
finally:
    os.close(alias)
    os.close(fd)
fd=os.eventfd(4,os.EFD_NONBLOCK|os.EFD_SEMAPHORE)
try:
    with select.epoll() as first, select.epoll() as second:
        for poller in (first,second):
            poller.register(fd,select.EPOLLOUT|select.EPOLLET)
            assert poller.poll(0)==[(fd,select.EPOLLOUT)]
            assert poller.poll(0)==[]
        for _ in range(2):
            assert os.eventfd_read(fd)==1
            for poller in (first,second):
                assert poller.poll(0.2)==[(fd,select.EPOLLOUT)], 'lost eventfd read wake'
                assert poller.poll(0)==[]
finally:os.close(fd)
libc.getrlimit64.argtypes=[I,P]
libc.setrlimit64.argtypes=[I,P]
page=libc.mmap(None,8192,3,0x22,-1,0)
assert page not in (None,C.c_void_p(-1).value)
try:
    assert libc.getrlimit64(6,page+1)==0  # unaligned buffers are valid
    assert libc.mprotect(page,4096,1)==0
    for destination in (page,1):
        C.set_errno(0)
        assert libc.getrlimit64(6,destination)==-1 and C.get_errno()==errno.EFAULT
    assert libc.mprotect(page,4096,0)==0
    C.set_errno(0)
    assert libc.setrlimit64(6,page)==-1 and C.get_errno()==errno.EFAULT
    assert libc.mprotect(page,4096,3)==0
    assert libc.mprotect(page+4096,4096,0)==0
    C.set_errno(0)
    assert libc.getrlimit64(6,page+4096-8)==-1 and C.get_errno()==errno.EFAULT
finally:assert libc.munmap(page,8192)==0
math=C.CDLL('libm.so.6',mode=C.RTLD_GLOBAL)
fixture=C.CDLL(str(Path(__file__).with_suffix('.so')))
gcc=C.CDLL('/usr/lib/libgcc_s.so.1')
class FindObject(C.Structure):
    _fields_=[('flags',C.c_ulonglong),('start',P),('end',P),('link_map',P),
              ('eh_frame',P),('reserved',C.c_ulonglong*7)]
assert C.sizeof(FindObject)==96
libc._dl_find_object.argtypes=[P,C.POINTER(FindObject)]
for symbol in (fixture.browser_unwind_probe,gcc._Unwind_Backtrace):
    address=C.cast(symbol,P).value
    info=FindObject()
    assert libc._dl_find_object(address,C.byref(info))==0
    assert info.start<=address<info.end and info.link_map and info.eh_frame
    assert C.string_at(info.eh_frame,1)==b'\x01'
    assert info.flags==0 and list(info.reserved)==[0]*7
assert libc._dl_find_object(1,C.byref(FindObject()))==-1
# A valid ELF mapping without PT_GNU_EH_FRAME still returns success.
no_header=C.CDLL(str(Path(__file__).with_name('BrowserStartupAbiProbe-no-header.so')))
address=C.cast(no_header.browser_guard_probe,P).value
info=FindObject()
assert libc._dl_find_object(address,C.byref(info))==0
assert info.start<=address<info.end and info.link_map and not info.eh_frame
fixture.browser_unwind_probe.argtypes=[P]
assert fixture.browser_unwind_probe(gcc._Unwind_Backtrace)==0
assert fixture.browser_fork_stack_probe()==0
assert fixture.browser_fs_redzone_probe()==0, 'TLS trampoline overwrote the SysV red zone'
result=fixture.browser_gs_unwind_probe()
assert result==0, ('stripped indirect GS function / switch case',result)
result=fixture.browser_gs_probe()
assert result==0, ('virtual Linux GS case',result)
result=fixture.browser_guard_probe()
assert result==0, ('incoherent guest stack guard',result)
result=fixture.browser_power_probe()
assert result==0, ('long-double power case',result)
result=fixture.browser_kernel_probe()
assert result==0, ('raw kernel capability probe',result)
libc.memfd_create.argtypes=[C.c_char_p,C.c_uint]
for name,flags in [(b'',0xffffffff),(b'probe',0x80000000),(b'x'*250,0)]:
    C.set_errno(errno.EPIPE)
    assert libc.memfd_create(name,flags)==-1
    assert C.get_errno()==errno.EINVAL
fd=libc.memfd_create(b'browser-abi',1)
assert fd>=0
try:
    import fcntl
    assert fcntl.fcntl(fd,fcntl.F_GETFD)&fcntl.FD_CLOEXEC
    assert os.write(fd,b'browser memfd')==13
    assert os.lseek(fd,0,os.SEEK_SET)==0
    assert os.read(fd,13)==b'browser memfd'
finally:os.close(fd)
# Chromium watches a thread through fstatat(/proc, "self/task/<tid>/").
import threading
import time
proc=os.open('/proc',os.O_RDONLY|os.O_DIRECTORY)
ready,release=threading.Event(),threading.Event()
thread_ids=[]
def task_probe():
    thread_ids.append(threading.get_native_id())
    ready.set()
    release.wait(10)
before=os.stat('self/task/',dir_fd=proc).st_nlink
assert before>=3
thread=threading.Thread(target=task_probe)
thread.start()
try:
    assert ready.wait(5)
    path='self/task/'+str(thread_ids[0])+'/'
    os.stat(path,dir_fd=proc)
    assert os.stat('self/task/',dir_fd=proc).st_nlink==before+1
finally:
    release.set();thread.join(5)
assert not thread.is_alive()
# Python's completion lock is released before the native pthread finishes its
# TLS destructors. Like Chromium, allow procfs a bounded retirement interval.
deadline=time.monotonic()+2
while True:
    try:os.stat(path,dir_fd=proc)
    except FileNotFoundError:break
    assert time.monotonic()<deadline, ('exited task remained in procfs',path)
    time.sleep(0.01)
for path in [path,'self/task/0/','self/task/not-a-tid/']:
    try:os.stat(path,dir_fd=proc)
    except FileNotFoundError:pass
    else:raise AssertionError(('nonexistent proc task',path))
assert os.stat('self/task/',dir_fd=proc).st_nlink==before
os.close(proc)
pthread=C.CDLL('libpthread.so.0')
pthread.pthread_mutexattr_setpshared.argtypes=[P,I]
for library in (libc,pthread):
    library.pthread_mutexattr_setpshared.argtypes=[P,I]
    attr=I(2) # PTHREAD_MUTEX_ERRORCHECK; pshared must preserve the type.
    assert library.pthread_mutexattr_setpshared(C.byref(attr),0)==0 and attr.value==2
    assert library.pthread_mutexattr_setpshared(C.byref(attr),1)==errno.ENOTSUP and attr.value==2
    assert library.pthread_mutexattr_setpshared(C.byref(attr),-1)==errno.EINVAL
    assert library.pthread_mutexattr_setpshared(None,0)==errno.EINVAL
x=C.CDLL('libX11.so.6')
x.XOpenDisplay.argtypes=[C.c_char_p];x.XOpenDisplay.restype=P
x.XCloseDisplay.argtypes=[P]
x._XGetScanlinePad.argtypes=[P,I];x._XGetBitsPerPixel.argtypes=[P,I]
x.XListExtensions.argtypes=[P,C.POINTER(I)];x.XListExtensions.restype=C.POINTER(C.c_char_p)
x.XFreeExtensionList.argtypes=[C.POINTER(C.c_char_p)]
x.XQueryExtension.argtypes=[P,C.c_char_p,C.POINTER(I),C.POINTER(I),C.POINTER(I)]
display=x.XOpenDisplay(None)
assert display
try:
    for depth,bits in [(1,1),(8,8),(16,16),(24,32),(32,32)]:
        assert x._XGetScanlinePad(display,depth)==32
        assert x._XGetBitsPerPixel(display,depth)==bits
    assert x._XGetBitsPerPixel(display,3)==4
    count=I()
    names=x.XListExtensions(display,C.byref(count))
    assert names and count.value>0
    try:
        assert b'XKEYBOARD' in [names[i] for i in range(count.value)]
        for i in range(count.value):
            opcode,event,error=I(),I(),I()
            assert x.XQueryExtension(display,names[i],C.byref(opcode),C.byref(event),C.byref(error))==1
    finally:x.XFreeExtensionList(names)
    assert x.XFreeExtensionList(None)==1
    class Providers(C.Structure):
        _fields_=[('timestamp',C.c_ulong),('count',I),('ids',C.POINTER(C.c_ulong))]
    class ProviderInfo(C.Structure):
        _fields_=[('capabilities',C.c_uint),('ncrtcs',I),('crtcs',C.POINTER(C.c_ulong)),
                  ('noutputs',I),('outputs',C.POINTER(C.c_ulong)),('name',C.c_char_p),
                  ('nassociated',I),('associated',P),('associated_capabilities',P),('name_len',I)]
    assert C.sizeof(Providers)==24 and C.sizeof(ProviderInfo)==72
    randr=C.CDLL('libXrandr.so.2')
    randr.XRRGetProviderResources.argtypes=[P,C.c_ulong];randr.XRRGetProviderResources.restype=C.POINTER(Providers)
    randr.XRRFreeProviderResources.argtypes=[C.POINTER(Providers)]
    randr.XRRGetScreenResourcesCurrent.argtypes=[P,C.c_ulong];randr.XRRGetScreenResourcesCurrent.restype=P
    randr.XRRFreeScreenResources.argtypes=[P]
    randr.XRRGetProviderInfo.argtypes=[P,P,C.c_ulong];randr.XRRGetProviderInfo.restype=C.POINTER(ProviderInfo)
    randr.XRRFreeProviderInfo.argtypes=[C.POINTER(ProviderInfo)]
    x.XDefaultRootWindow.argtypes=[P];x.XDefaultRootWindow.restype=C.c_ulong
    root=x.XDefaultRootWindow(display)
    providers=randr.XRRGetProviderResources(display,root)
    assert providers and providers.contents.count in (0,1)
    try:
        if providers.contents.count:
            resources=randr.XRRGetScreenResourcesCurrent(display,root)
            assert resources
            try:
                info=randr.XRRGetProviderInfo(display,resources,providers.contents.ids[0])
                assert info
                try:
                    assert info.contents.capabilities==0 and info.contents.nassociated==0
                    assert info.contents.ncrtcs==1 and info.contents.crtcs[0]==1
                    assert info.contents.noutputs==1 and info.contents.outputs[0]==1
                    assert len(info.contents.name)==info.contents.name_len
                finally:randr.XRRFreeProviderInfo(info)
            finally:randr.XRRFreeScreenResources(resources)
    finally:randr.XRRFreeProviderResources(providers)
    randr.XRRFreeProviderInfo(None)
    randr.XRRFreeProviderResources(None)
finally:x.XCloseDisplay(display)

xcb=C.CDLL('libxcb.so.1')
xcb.xcb_connect.argtypes=[C.c_char_p,C.POINTER(I)];xcb.xcb_connect.restype=P
xcb.xcb_disconnect.argtypes=[P]
xcb.xcb_connection_has_error.argtypes=[P]
xcb.xcb_send_fd.argtypes=[P,I];xcb.xcb_send_fd.restype=None
xcb.xcb_send_request_with_fds.argtypes=[P,I,P,P,C.c_uint,C.POINTER(I)]
xcb.xcb_flush.argtypes=[P]
for batched in (False,True):
    connection=xcb.xcb_connect(None,None)
    assert connection and xcb.xcb_connection_has_error(connection)==0
    fd=os.open('/dev/null',os.O_RDONLY)
    try:
        if batched:
            descriptors=(I*1)(fd)
            assert xcb.xcb_send_request_with_fds(connection,0,None,None,1,descriptors)==0
        else:xcb.xcb_send_fd(connection,fd)
        assert xcb.xcb_connection_has_error(connection)==7
        assert xcb.xcb_flush(connection)==0
        try:os.fstat(fd)
        except OSError as error:assert error.errno==errno.EBADF
        else:raise AssertionError('XCB did not consume the transferred descriptor')
    finally:xcb.xcb_disconnect(connection)
print('BROWSER_STARTUP_ABI_OK',flush=True)
