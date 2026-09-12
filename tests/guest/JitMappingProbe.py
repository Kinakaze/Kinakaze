"""SpiderMonkey-style fixed PROT_NONE recycling, with live neighboring pages."""
import ctypes as c
lib=c.CDLL('libc.so.6',use_errno=True)
mmap=lib.mmap;mmap.restype=c.c_void_p;mmap.argtypes=[c.c_void_p,c.c_size_t,c.c_int,c.c_int,c.c_int,c.c_longlong]
mprotect=lib.mprotect;mprotect.restype=c.c_int;mprotect.argtypes=[c.c_void_p,c.c_size_t,c.c_int]
munmap=lib.munmap;munmap.restype=c.c_int;munmap.argtypes=[c.c_void_p,c.c_size_t]
page=4096;size=65536
base=mmap(None,size,0,0x22,-1,0);assert base not in (None,c.c_void_p(-1).value),c.get_errno()
assert mmap(base,size,3,0x32,-1,0)==base,c.get_errno()
c.memset(base,0xa5,size)
assert mprotect(base+page,page,5)==0
assert mprotect(base+3*page,page,1)==0
for _ in range(3):
    target=base+page
    assert mmap(target,3*page,0,0x32,-1,0)==target,c.get_errno()
    assert c.c_ubyte.from_address(base).value==0xa5
    assert c.c_ubyte.from_address(base+4*page).value==0xa5
    assert mprotect(target,3*page,3)==0,c.get_errno()
    assert bytes((c.c_ubyte*(3*page)).from_address(target))==b'\0'*(3*page)
    c.memset(target,0x7e,3*page)
assert mmap(base,size,0,0x32,-1,0)==base,c.get_errno()
assert mmap(base,size,3,0x32,-1,0)==base,c.get_errno()
assert bytes((c.c_ubyte*size).from_address(base))==b'\0'*size
assert munmap(base,size)==0
print('JIT_FIXED_PROT_NONE_RECYCLE_OK',flush=True)
