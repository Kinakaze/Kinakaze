"""Shared Xlib/XCB controls, eventfd ownership and window operations."""
import ctypes as c
import os
import select
import struct
import threading
import time

P,U,I,B,S,H=c.c_void_p,c.c_uint,c.c_int,c.c_ubyte,c.c_ushort,c.c_short
x=c.CDLL('libX11.so.6');b=c.CDLL('libxcb.so.1');lib=c.CDLL('libc.so.6')
def bind(l,n,r,a):
    f=getattr(l,n);f.restype=r;f.argtypes=a;return f
free=bind(lib,'free',None,[P])
display=bind(x,'XOpenDisplay',P,[P])(None)
conn=bind(b,'xcb_connect',P,[P,P])(None,None)
assert conn==bind(c.CDLL('libX11-xcb.so.1'),'XGetXCBConnection',P,[P])(display)
assert conn==bind(b,'XGetXCBConnection',P,[P])(display)
fd=bind(b,'xcb_get_file_descriptor',I,[P])(conn)
assert fd==bind(x,'XConnectionNumber',I,[P])(display) and fd>=0
check=bind(b,'xcb_request_check',P,[P,U])
def checked(seq,code=0):
    e=check(conn,seq)
    if code:
        assert e
        raw=c.string_at(e,36);free(e);assert raw[1]==code,raw
    else:
        assert not e,c.string_at(e,36).hex() if e else ''
def response(f,seq):
    e=P();a=f(conn,seq,c.byref(e));assert a and not e.value
    header=c.string_at(a,32);n=struct.unpack_from('<I',header,4)[0]*4
    data=c.string_at(a,32+n);free(a);return data
control=bind(b,'xcb_change_keyboard_control_checked',U,[P,U,P])
query=bind(b,'xcb_get_keyboard_control',U,[P])
reply=bind(b,'xcb_get_keyboard_control_reply',P,[P,U,P])
def controls(mask,values):checked(control(conn,mask,(U*len(values))(*values)))
controls(255,[0,0,880,5,2,1,38,0])
def verify():
    raw=response(reply,query(conn));assert len(raw)==52
    assert raw[1]==1 and struct.unpack_from('<I',raw,8)[0]==2
    assert struct.unpack_from('<BBHH',raw,12)==(0,0,880,5)
    assert raw[20+38//8] & (1<<(38%8))==0
verify()
checked(control(conn,16,(U*1)(2)),8)
checked(control(conn,2,(U*1)(101)),2)
bell=bind(b,'xcb_bell_checked',U,[P,c.c_byte])
checked(bell(conn,-100));checked(bell(conn,127),2)
child=os.fork()
if child==0:
    try:
        verify();controls(192,[38,1]);assert response(reply,query(conn))[24] & 64
    except BaseException:
        import traceback;traceback.print_exc();os._exit(1)
    os._exit(0)
assert os.waitpid(child,0)==(child,0);verify()

gen=bind(b,'xcb_generate_id',U,[P])
create=bind(b,'xcb_create_window_checked',U,[P,B,U,U,H,H,S,S,S,S,U,U,P])
win,parent=gen(conn),gen(conn)
checked(create(conn,24,parent,1,20,30,160,120,0,1,1,0,None))
checked(create(conn,24,win,1,10,20,64,48,0,1,1,0,None))
configure=bind(b,'xcb_configure_window_checked',U,[P,U,S,P])
checked(configure(conn,win,12,(U*2)(80,60)))
geometry=bind(b,'xcb_get_geometry',U,[P,U]);geometry_reply=bind(b,'xcb_get_geometry_reply',P,[P,U,P])
size=struct.unpack_from('<HH',response(geometry_reply,geometry(conn,win)),16);assert size==(80,60),size
reparent=bind(b,'xcb_reparent_window_checked',U,[P,U,U,H,H])
checked(reparent(conn,win,parent,3,4))
tree=bind(b,'xcb_query_tree',U,[P,U]);tree_reply=bind(b,'xcb_query_tree_reply',P,[P,U,P])
assert struct.unpack_from('<I',response(tree_reply,tree(conn,win)),12)[0]==parent
attrs=bind(b,'xcb_change_window_attributes_checked',U,[P,U,U,P])
checked(attrs(conn,win,2,(U*1)(0x102345)))
clear=bind(b,'xcb_clear_area_checked',U,[P,B,U,H,H,S,S])
checked(clear(conn,0,win,0,0,0,0))
image=bind(b,'xcb_get_image',U,[P,B,U,H,H,S,S,U]);image_reply=bind(b,'xcb_get_image_reply',P,[P,U,P])
assert struct.unpack_from('<4I',response(image_reply,image(conn,2,win,0,0,2,2,0xffffffff)),32)==(0x102345,)*4
poll=bind(b,'xcb_poll_for_event',P,[P])
while (e:=poll(conn)):free(e)
assert not select.select([fd],[],[],0)[0]
checked(clear(conn,1,win,1,2,3,4))
assert select.select([fd],[],[],0)[0]==[fd]
e=poll(conn);assert e;raw=c.string_at(e,36);free(e)
assert raw[0]==12 and struct.unpack_from('<IhhHH',raw,4)==(win,1,2,3,4)
while (e:=poll(conn)):free(e)
assert not select.select([fd],[],[],0)[0]

# A blocked consumer wakes on a generated protocol event without timer polling.
wait=bind(b,'xcb_wait_for_event',P,[P]);received=[]
def receive():
    while True:
        e=wait(conn);assert e;raw=c.string_at(e,36);free(e)
        if raw[0]==12:received.append(raw);return
t=threading.Thread(target=receive);t.start();time.sleep(0.025)
checked(clear(conn,1,win,0,0,1,1));t.join()
assert not t.is_alive() and received and received[0][0]==12,(t.is_alive(),received)

focus=bind(b,'xcb_set_input_focus_checked',U,[P,B,U,U])
focus_get=bind(b,'xcb_get_input_focus',U,[P]);focus_reply=bind(b,'xcb_get_input_focus_reply',P,[P,U,P])
checked(focus(conn,0,win,0),8)  # unmapped windows cannot receive focus
checked(focus(conn,0,0,0))
assert struct.unpack_from('<I',response(focus_reply,focus_get(conn)),8)[0]==0
checked(focus(conn,2,1,0))
r=response(focus_reply,focus_get(conn));assert r[1]==2 and struct.unpack_from('<I',r,8)[0]==1
checked(focus(conn,3,1,0),2)
warp=bind(b,'xcb_warp_pointer_checked',U,[P,U,U,H,H,S,S,H,H])
checked(warp(conn,0,0xdead,0,0,0,0,0,0),3)
# A point outside the root's source rectangle leaves the user's cursor untouched.
checked(warp(conn,1,0,-30000,-30000,1,1,0,0))
destroy=bind(b,'xcb_destroy_window',U,[P,U]);destroy(conn,win);destroy(conn,parent)
controls(143,[0,50,400,100,2]);controls(240,[2,0,38,2])
bind(b,'xcb_disconnect',None,[P])(conn)
assert bind(x,'XConnectionNumber',I,[P])(display)==fd
bind(x,'XCloseDisplay',I,[P])(display)
try:os.fstat(fd);raise AssertionError('eventfd leaked')
except OSError as e:assert e.errno==9
print('XCB_CONTROLS_EVENTFD_WAIT_WINDOW_FOCUS_FORK_OK')
