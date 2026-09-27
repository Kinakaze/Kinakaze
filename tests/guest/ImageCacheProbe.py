"""Verify on-demand image reuse across mutation, fork, rename and fs-verity."""
import _ctypes
import ctypes
import fcntl
import os
from pathlib import Path
import struct
import sys

SIZE=16*1024*1024+37
MID=8*1024*1024
original=Path(sys.argv[1]).read_bytes()
phoff,shoff=struct.unpack_from('<QQ',original,32)
phsize,phcount,shsize,shcount=struct.unpack_from('<HHHH',original,54)
sections=[struct.unpack_from('<IIQQQQIIQQ',original,shoff+i*shsize) for i in range(shcount)]
symbol=None
for section in sections:
    if section[1]!=11:continue
    strings=sections[section[6]]
    names=original[strings[4]:strings[4]+strings[5]]
    for at in range(section[4],section[4]+section[5],section[9]):
        name,info,other,index,value,size=struct.unpack_from('<IBBHQQ',original,at)
        if names[name:].split(b'\0',1)[0]==b'image_pages':symbol=value
assert symbol is not None
file_offset=None
for i in range(phcount):
    kind,flags,offset,vaddr,paddr,filesz,memsz,align=struct.unpack_from('<IIQQQQQQ',original,phoff+i*phsize)
    if kind==1 and vaddr<=symbol< vaddr+filesz:file_offset=offset+symbol-vaddr
assert file_offset is not None and original[file_offset+MID]==91

def load(path):
    library=ctypes.CDLL(str(path))
    value=library.image_value;value.argtypes=[ctypes.c_uint];value.restype=ctypes.c_int
    return library,value
def check(value,middle):
    for at,expected in [(0,3),(1,0),(MID,middle),(SIZE-1,171)]:assert value(at)==expected,(at,value(at),expected)
def close(library,value):
    del value
    _ctypes.dlclose(library._handle)

path=Path(sys.argv[2]);expected=int(sys.argv[3]);mode=sys.argv[4]
if mode=='reject':
    try:invalid=ctypes.CDLL(str(path))
    except OSError:print('IMAGE_CACHE_REJECTED',flush=True);sys.exit(0)
    raise AssertionError('invalid image was accepted')

lib,value=load(path);check(value,expected)
if mode=='mutate':
    stamp=path.stat()
    with path.open('r+b') as writer:
        writer.seek(file_offset+MID);writer.write(bytes([92]));writer.flush()
    os.utime(path,ns=(stamp.st_atime_ns,stamp.st_mtime_ns))
    check(value,expected) # Existing immutable image survives file mutation.
elif mode=='rename':
    renamed=path.with_suffix('.old');path.rename(renamed);path.write_bytes(original)
    renamed.unlink();check(value,expected)
elif mode=='truncate':
    with path.open('r+b') as writer:writer.truncate(8192)
    check(value,expected)
elif mode=='verity':
    # Converting a previously cached ordinary inode to fs-verity must switch
    # back to logical-length, block-verified image reads (no hidden Merkle tail).
    fd=os.open(path,os.O_RDONLY)
    try:
        enable=bytearray(128)
        struct.pack_into('<IIII',enable,0,1,1,4096,0)
        fcntl.ioctl(fd,0x40806685,enable)
    finally:os.close(fd)
    check(value,expected)
else:assert mode=='read',mode
child=os.fork()
if child==0:
    check(value,expected);os._exit(0)
assert os.waitpid(child,0)[1]==0
check(value,expected)
print('IMAGE_CACHE_SNAPSHOT_OK',mode,flush=True)
