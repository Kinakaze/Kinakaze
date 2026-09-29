"""Actual memory contents, omitted ranges, zero-on-fork, dump policy and reuse."""
import ctypes as C
import errno
import os
import traceback

lib=C.CDLL(None,use_errno=True)
lib.mmap.argtypes=[C.c_void_p,C.c_size_t,C.c_int,C.c_int,C.c_int,C.c_longlong]
lib.mmap.restype=C.c_void_p
lib.madvise.argtypes=[C.c_void_p,C.c_size_t,C.c_int]
lib.munmap.argtypes=[C.c_void_p,C.c_size_t]
lib.mincore.argtypes=[C.c_void_p,C.c_size_t,C.c_void_p]
page=4096
address=lib.mmap(None,5*page,3,0x22,-1,0)
assert address!=C.c_void_p(-1).value,C.get_errno()
def advice(offset,length,value):
    assert lib.madvise(address+offset,length,value)==0,(value,C.get_errno())
def child(check):
    pid=os.fork()
    if pid==0:
        try: check()
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    assert os.waitpid(pid,0)==(pid,0)
try:
    C.memset(address,65,5*page)
    advice(0,5*page,16)
    advice(page,2*page,17)
    advice(page,2*page,16)
    assert C.string_at(address,5*page)==b'A'*(5*page)
    assert lib.madvise(address+1,page,16)==-1 and C.get_errno()==errno.EINVAL
    advice(page,2*page,10)
    advice(3*page,page,18)
    def first_child():
        assert C.string_at(address,1)==b'A'
        assert C.string_at(address+4*page,1)==b'A'
        vector=C.c_ubyte()
        assert lib.mincore(address+page,page,C.byref(vector))==-1 and C.get_errno()==errno.ENOMEM
        assert C.string_at(address+3*page,page)==b'\0'*page
        C.memset(address+3*page,66,page)
        def grandchild():
            assert C.string_at(address+3*page,page)==b'\0'*page
        child(grandchild)
        assert C.string_at(address+3*page,1)==b'B'
    child(first_child)
    assert C.string_at(address,5*page)==b'A'*(5*page)
    advice(page,2*page,11)
    advice(3*page,page,19)
    def second_child():
        assert C.string_at(address,5*page)==b'A'*(5*page)
        advice(0,5*page,17)
    child(second_child)
    advice(0,5*page,17)
finally:
    assert lib.munmap(address,5*page)==0,C.get_errno()
print('MADVISE_DUMP_FORK_ZERO_RESTORE_OK',flush=True)
