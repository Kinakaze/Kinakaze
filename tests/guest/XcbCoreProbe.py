"""XCB/Xlib shared properties, independent wire replies and fork reconstruction."""
import ctypes as c
import os
import struct

x=c.CDLL('libX11.so.6');b=c.CDLL('libxcb.so.1');lib=c.CDLL('libc.so.6')
P=c.c_void_p;U=c.c_uint;I=c.c_int;B=c.c_ubyte;S=c.c_ushort
def bind(lib,name,restype,args):
    f=getattr(lib,name);f.restype=restype;f.argtypes=args;return f
free=bind(lib,'free',None,[P])
display=bind(x,'XOpenDisplay',P,[P])(None);assert display
connect=bind(b,'xcb_connect',P,[P,P]);conn=connect(None,None);assert conn
intern_x=bind(x,'XInternAtom',c.c_ulong,[P,c.c_char_p,I])
intern=bind(b,'xcb_intern_atom',U,[P,B,S,c.c_char_p])
intern_reply=bind(b,'xcb_intern_atom_reply',P,[P,U,P])
atom_query=bind(b,'xcb_get_atom_name',U,[P,U]);atom_reply=bind(b,'xcb_get_atom_name_reply',P,[P,U,P])
change=bind(b,'xcb_change_property_checked',U,[P,B,U,U,U,B,U,P])
get=bind(b,'xcb_get_property',U,[P,B,U,U,U,U,U]);get_reply=bind(b,'xcb_get_property_reply',P,[P,U,P])
check=bind(b,'xcb_request_check',P,[P,U])
poll=bind(b,'xcb_poll_for_reply',I,[P,U,P,P])
discard=bind(b,'xcb_discard_reply',None,[P,U])
def response(call,sequence):
    error=P(123);address=call(conn,sequence,c.byref(error))
    assert address and not error.value,(call,sequence,error.value)
    data=c.string_at(address,32);length=struct.unpack_from('<I',data,4)[0]
    assert data[0]==1 and struct.unpack_from('<H',data,2)[0]==sequence&65535
    data=c.string_at(address,32+length*4);free(address);return data
def atom(name):
    return struct.unpack_from('<I',response(intern_reply,intern(conn,0,len(name),name)),8)[0]

assert atom(b'PRIMARY')==1 and atom(b'CARDINAL')==6
a=intern(conn,0,8,b'xcb-name');z=intern(conn,0,9,b'xcb-other');assert a!=z
other=struct.unpack_from('<I',response(intern_reply,z),8)[0]
name=struct.unpack_from('<I',response(intern_reply,a),8)[0]
assert name!=other and name==intern_x(display,b'xcb-name',0)
assert atom(b'xcb-name')==name
missing=intern(conn,1,19,b'xcb-never-created-42')
assert struct.unpack_from('<I',response(intern_reply,missing),8)[0]==0
r=response(atom_reply,atom_query(conn,name));assert r[32:40]==b'xcb-name' and struct.unpack_from('<H',r,8)[0]==8
error=P();seq=atom_query(conn,0xffffffff);assert not atom_reply(conn,seq,c.byref(error))
assert error.value and c.string_at(error,2)==b'\0\5';free(error)
property_id=atom(b'KINAKAZE_XCB_PROPERTY')
words=(U*4)(0,0x12345678,0x80000000,0xffffffff)
assert not check(conn,change(conn,0,1,property_id,6,32,4,words))
xp=bind(x,'XGetWindowProperty',I,[P,c.c_ulong,c.c_ulong,c.c_long,c.c_long,I,c.c_ulong,P,P,P,P,P])
kind=c.c_ulong();fmt=I();count=c.c_ulong();after=c.c_ulong();values=P()
assert xp(display,1,property_id,0,4,0,6,c.byref(kind),c.byref(fmt),c.byref(count),c.byref(after),c.byref(values))==0
assert (kind.value,fmt.value,count.value,after.value)==(6,32,4,0)
assert [v&0xffffffff for v in c.cast(values,c.POINTER(c.c_ulong*4)).contents]==list(words);free(values)
slice_cookie=get(conn,0,1,property_id,6,1,2)
r=response(get_reply,slice_cookie)
assert (r[1],struct.unpack_from('<III',r,8),struct.unpack_from('<II',r,32))==(32,(6,4,2),(0x12345678,0x80000000))
assert len(r)==40
wrong=response(get_reply,get(conn,0,1,property_id,31,0,9))
assert struct.unpack_from('<III',wrong,8)==(6,16,0)
bad=check(conn,change(conn,0,1,property_id,6,7,0,None));assert bad and c.string_at(bad,2)==b'\0\2';free(bad)
pending=atom_query(conn,name);answer=P();error=P()
assert poll(conn,pending,c.byref(answer),c.byref(error))==1 and answer.value and not error.value;free(answer)
assert poll(conn,pending,c.byref(answer),c.byref(error))==0 and not answer.value
pending=atom_query(conn,name);discard(conn,pending)
assert poll(conn,pending,c.byref(answer),c.byref(error))==0

generate=bind(b,'xcb_generate_id',U,[P]);window=generate(conn);assert window!=1
create=bind(b,'xcb_create_window_checked',U,[P,B,U,U,c.c_short,c.c_short,S,S,S,S,U,U,P])
assert not check(conn,create(conn,24,window,1,17,29,320,180,0,1,1,0,None))
geometry=bind(b,'xcb_get_geometry',U,[P,U]);geometry_reply=bind(b,'xcb_get_geometry_reply',P,[P,U,P])
attrs=bind(b,'xcb_get_window_attributes',U,[P,U]);attrs_reply=bind(b,'xcb_get_window_attributes_reply',P,[P,U,P])
tree=bind(b,'xcb_query_tree',U,[P,U]);tree_reply=bind(b,'xcb_query_tree_reply',P,[P,U,P])
def window_checks():
    r=response(geometry_reply,geometry(conn,window));assert struct.unpack_from('<HH',r,16)==(320,180)
    r=response(attrs_reply,attrs(conn,window));assert len(r)==44 and r[26]==0
    r=response(tree_reply,tree(conn,1));n=struct.unpack_from('<H',r,16)[0]
    assert window in struct.unpack_from('<'+'I'*n,r,32)
window_checks()
assert not check(conn,change(conn,0,window,property_id,6,32,4,words))

# Both parent and child consume their own retained replies; resource IDs are
# rebound to new HWNDs, and window properties follow the stable logical window.
saved=atom_query(conn,name);saved_property=get(conn,0,window,property_id,6,0,4)
child=os.fork()
if child==0:
    try:
        window_checks()
        assert response(atom_reply,saved)[32:40]==b'xcb-name'
        assert struct.unpack_from('<IIII',response(get_reply,saved_property),32)==tuple(words)
        assert struct.unpack_from('<IIII',response(get_reply,get(conn,0,window,property_id,6,0,4)),32)==tuple(words)
        changed=(U*1)(99);assert not check(conn,change(conn,0,window,property_id,6,32,1,changed))
        os._exit(0)
    except BaseException:
        import traceback;traceback.print_exc();os._exit(77)
assert os.waitpid(child,0)==(child,0)
assert response(atom_reply,saved)[32:40]==b'xcb-name'
assert struct.unpack_from('<IIII',response(get_reply,saved_property),32)==tuple(words)
assert struct.unpack_from('<IIII',response(get_reply,get(conn,0,window,property_id,6,0,4)),32)==tuple(words)

keyboard=bind(b,'xcb_get_keyboard_mapping',U,[P,B,B]);keyboard_reply=bind(b,'xcb_get_keyboard_mapping_reply',P,[P,U,P])
r=response(keyboard_reply,keyboard(conn,38,1));assert r[1]>0 and struct.unpack_from('<I',r,32)[0] in (ord('a'),ord('A'))
modifiers=bind(b,'xcb_get_modifier_mapping',U,[P]);modifiers_reply=bind(b,'xcb_get_modifier_mapping_reply',P,[P,U,P])
r=response(modifiers_reply,modifiers(conn));assert r[1]==2 and 50 in r[32:48]
for _ in range(100):
    response(intern_reply,intern(conn,0,8,b'xcb-name'))
    response(get_reply,get(conn,0,window,property_id,6,0,4))
bind(b,'xcb_destroy_window',U,[P,U])(conn,window)
bind(b,'xcb_disconnect',None,[P])(conn)
bind(x,'XCloseDisplay',I,[P])(display)
print('XCB_SHARED_PROPERTIES_QUERIES_FORK_OWNERSHIP_OK')
