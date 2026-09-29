import ctypes, os, select, time, tempfile, json
libc=ctypes.CDLL(None,use_errno=True)
libc.inotify_add_watch.argtypes=[ctypes.c_int,ctypes.c_char_p,ctypes.c_uint32]
for kind in ('timer','inotify'):
    with tempfile.TemporaryDirectory() as directory, select.epoll() as ep:
        fds=[]
        try:
            for i in range(16):
                fd=libc.timerfd_create(1,os.O_NONBLOCK) if kind=='timer' else libc.inotify_init1(os.O_NONBLOCK)
                assert fd>=0
                fds.append(fd)
                if kind=='inotify': assert libc.inotify_add_watch(fd,os.fsencode(directory),0x100)>=0
                ep.register(fd,select.EPOLLIN)
            for run in range(3):
                cpu=time.process_time(); start=time.monotonic()
                assert ep.poll(.75)==[]
                print(json.dumps(dict(kind=kind,run=run,cpu_ms=(time.process_time()-cpu)*1000,elapsed_ms=(time.monotonic()-start)*1000)),flush=True)
        finally:
            for fd in fds: os.close(fd)
print('WAIT_IDLE_PASS',flush=True)
