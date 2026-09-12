"""Selection handshakes and real cursor/colormap resources, including fork."""
import ctypes as c
import os
import struct
P,U,I,B,S,H=c.c_void_p,c.c_uint,c.c_int,c.c_ubyte,c.c_ushort,c.c_short
b=c.CDLL('libxcb.so.1');x=c.CDLL('libX11.so.6');lib=c.CDLL('libc.so.6')
def bind(l,n,r,a):
    f=getattr(l,n);f.restype=r;f.argtypes=a;return f
free=bind(lib,'free',None,[P]);display=bind(x,'XOpenDisplay',P,[P])(None)
conn=bind(b,'xcb_connect',P,[P,P])(None,None)
check=bind(b,'xcb_request_check',P,[P,U])
def checked(seq,code=0):
    e=check(conn,seq)
    if code:
        assert e
        raw=c.string_at(e,36);free(e);assert raw[1]==code,(raw,code)
    else:assert not e,c.string_at(e,36).hex() if e else ''
def response(fn,seq):
    error=P();p=fn(conn,seq,c.byref(error));assert p and not error.value
    header=c.string_at(p,32);length=struct.unpack_from('<I',header,4)[0]*4
    raw=c.string_at(p,32+length);free(p);return raw
poll=bind(b,'xcb_poll_for_event',P,[P])
def event(kind):
    while (p:=poll(conn)):
        raw=c.string_at(p,36);free(p)
        if raw[0]&127==kind:return raw
    raise AssertionError(('missing event',kind))
generate=bind(b,'xcb_generate_id',U,[P])
intern=bind(b,'xcb_intern_atom',U,[P,B,S,c.c_char_p]);intern_reply=bind(b,'xcb_intern_atom_reply',P,[P,U,P])
def atom(name):return struct.unpack_from('<I',response(intern_reply,intern(conn,0,len(name),name)),8)[0]
selection,target,prop=atom(b'KINAKAZE_SELECTION'),atom(b'UTF8_STRING'),atom(b'KINAKAZE_TRANSFER')
owner,requestor=generate(conn),generate(conn)
create=bind(b,'xcb_create_window_checked',U,[P,B,U,U,H,H,S,S,S,S,U,U,P])
for window in (owner,requestor):checked(create(conn,24,window,1,0,0,32,32,0,1,1,0,None))
set_owner=bind(b,'xcb_set_selection_owner_checked',U,[P,U,U,U])
get_owner=bind(b,'xcb_get_selection_owner',U,[P,U]);owner_reply=bind(b,'xcb_get_selection_owner_reply',P,[P,U,P])
def query_owner():return struct.unpack_from('<I',response(owner_reply,get_owner(conn,selection)),8)[0]
convert=bind(b,'xcb_convert_selection_checked',U,[P,U,U,U,U,U])
checked(convert(conn,requestor,selection,target,prop,0))
notify=event(31);assert struct.unpack_from('<IIII',notify,8)==(requestor,selection,target,0)
checked(set_owner(conn,owner,selection,0));assert query_owner()==owner
assert bind(x,'XGetSelectionOwner',c.c_ulong,[P,c.c_ulong])(display,selection)!=0
checked(convert(conn,requestor,selection,target,prop,0))
request=event(30)
assert struct.unpack_from('<IIIII',request,8)==(owner,requestor,selection,target,prop)
change=bind(b,'xcb_change_property_checked',U,[P,B,U,U,U,B,U,P])
payload=b'XCB selection UTF8 \xe4\xbd\xa0\xe5\xa5\xbd'
checked(change(conn,0,requestor,prop,target,8,len(payload),c.c_char_p(payload)))
send=bind(b,'xcb_send_event_checked',U,[P,B,U,U,P])
reply_event=bytearray(32);reply_event[0]=31
struct.pack_into('<IIIII',reply_event,4,struct.unpack_from('<I',request,4)[0],requestor,selection,target,prop)
checked(send(conn,0,requestor,0,c.c_char_p(bytes(reply_event))))
notify=event(31);assert notify[0]==159 and struct.unpack_from('<IIII',notify,8)==(requestor,selection,target,prop)
get_property=bind(b,'xcb_get_property',U,[P,B,U,U,U,U,U]);property_reply=bind(b,'xcb_get_property_reply',P,[P,U,P])
raw=response(property_reply,get_property(conn,0,requestor,prop,target,0,100));assert raw[32:32+len(payload)]==payload
checked(set_owner(conn,requestor,selection,0));assert query_owner()==requestor
clear=event(29);assert struct.unpack_from('<II',clear,8)==(owner,selection)

colormap=generate(conn)
create_map=bind(b,'xcb_create_colormap_checked',U,[P,B,U,U,U])
free_map=bind(b,'xcb_free_colormap_checked',U,[P,U])
checked(create_map(conn,0,colormap,1,1));checked(create_map(conn,0,colormap,1,1),14)
attrs=bind(b,'xcb_change_window_attributes_checked',U,[P,U,U,P])
checked(attrs(conn,requestor,1<<13,(U*1)(colormap)))
get_attrs=bind(b,'xcb_get_window_attributes',U,[P,U]);attrs_reply=bind(b,'xcb_get_window_attributes_reply',P,[P,U,P])
assert struct.unpack_from('<I',response(attrs_reply,get_attrs(conn,requestor)),28)[0]==colormap

pixmap,gc,cursor=generate(conn),generate(conn),generate(conn)
create_pixmap=bind(b,'xcb_create_pixmap_checked',U,[P,B,U,U,S,S])
create_gc=bind(b,'xcb_create_gc_checked',U,[P,U,U,U,P])
put=bind(b,'xcb_put_image_checked',U,[P,B,U,U,S,S,H,H,B,B,U,P])
checked(create_pixmap(conn,1,pixmap,1,8,8));checked(create_gc(conn,gc,pixmap,0,None))
bits=b''.join(bytes([1<<i,0,0,0]) for i in range(8))
checked(put(conn,2,pixmap,gc,8,8,0,0,0,1,len(bits),c.c_char_p(bits)))
create_cursor=bind(b,'xcb_create_cursor_checked',U,[P,U,U,U,S,S,S,S,S,S,S,S])
free_cursor=bind(b,'xcb_free_cursor_checked',U,[P,U])
checked(create_cursor(conn,cursor,pixmap,0,65535,0,0,0,0,65535,1,2))
checked(attrs(conn,requestor,1<<14,(U*1)(cursor)))
# The selected window retains its cursor after the caller frees the resource ID.
checked(free_cursor(conn,cursor));checked(free_cursor(conn,cursor),6)
bind(b,'xcb_free_gc',U,[P,U])(conn,gc)
checked(bind(b,'xcb_free_pixmap_checked',U,[P,U])(conn,pixmap))

# libXcursor creates server resources through Xlib; XCB must accept the same ID.
class CursorImage(c.Structure):
    _fields_=[('version',U),('size',U),('width',U),('height',U),('xhot',U),('yhot',U),('delay',U),('pixels',P)]
cursor_lib=c.CDLL('libXcursor.so.1')
pixels=(U*4)(0xffff0000,0xff00ff00,0xff0000ff,0)
image=CursorImage(1,2,2,2,0,0,0,c.cast(pixels,P))
shared_cursor=bind(cursor_lib,'XcursorImageLoadCursor',c.c_ulong,[P,P])(display,c.byref(image))
assert shared_cursor
checked(attrs(conn,owner,1<<14,(U*1)(shared_cursor)))
bind(x,'XFreeCursor',I,[P,c.c_ulong])(display,shared_cursor)
checked(attrs(conn,requestor,1<<14,(U*1)(shared_cursor)),6)

# SendEvent recipient masks do not depend on the payload's event type.
message=bytearray(32);message[0]=33;message[1]=32
struct.pack_into('<II',message,4,owner,prop)
struct.pack_into('<IIIII',message,12,0x12345678,2,3,4,5)
wire=c.c_char_p(bytes(message))
mask=1<<19
checked(send(conn,0,owner,mask,wire));assert not poll(conn)
checked(send(conn,0,1,(1<<19)|(1<<20),wire));assert not poll(conn)
checked(attrs(conn,owner,1<<11,(U*1)(mask)))
checked(send(conn,0,owner,mask|(1<<20),wire))
delivered=event(33);assert delivered[0]==161 and delivered[4:32]==message[4:32]
checked(send(conn,0,owner,1<<20,wire));assert not poll(conn)
checked(send(conn,0,owner,1<<25,wire),2)
checked(send(conn,0,0xdeadbeef,mask,wire),3)
checked(attrs(conn,owner,1<<11,(U*1)(0)))

# Xlib uses the same subscriber state. Validate payload and send_event through XCB.
xcreate=bind(x,'XCreateSimpleWindow',c.c_ulong,[P,c.c_ulong,I,I,U,U,U,c.c_ulong,c.c_ulong])
xwindow=xcreate(display,1,0,0,32,32,0,0,0)
xselect=bind(x,'XSelectInput',I,[P,c.c_ulong,c.c_long])
xsend=bind(x,'XSendEvent',I,[P,c.c_ulong,I,c.c_long,P])
class ClientMessage(c.Structure):
    _fields_=[('kind',I),('serial',c.c_ulong),('sent',I),('display',P),('window',c.c_ulong),('message_type',c.c_ulong),('format',I),('data',c.c_long*5)]
xevent=c.create_string_buffer(192)
client=ClientMessage.from_buffer(xevent)
client.kind=33;client.display=display;client.window=xwindow;client.message_type=prop;client.format=32
client.data[:]=[0x12345678,2,3,4,5]
assert xsend(display,xwindow,0,mask,xevent)==1 and not poll(conn)
assert xselect(display,xwindow,mask)==1
assert xsend(display,xwindow,0,mask,xevent)==1
delivered=event(33);assert delivered[0]==161 and delivered[12:32]==message[12:32]
assert xselect(display,xwindow,0)==1
bind(x,'XDestroyWindow',I,[P,c.c_ulong])(display,xwindow)

# Timestamp queries depend on PropertyNotify, without any timer or event polling shim.
checked(attrs(conn,owner,1<<11,(U*1)(1<<22)))
checked(change(conn,0,owner,prop,target,8,len(payload),c.c_char_p(payload)))
notice=event(28)
assert struct.unpack_from('<II',notice,4)==(owner,prop) and notice[16]==0
assert struct.unpack_from('<I',notice,12)[0]!=0
delete=bind(b,'xcb_delete_property_checked',U,[P,U,U])
checked(delete(conn,owner,prop));assert event(28)[16]==1
checked(delete(conn,owner,prop));assert not poll(conn)
checked(attrs(conn,owner,1<<11,(U*1)(0)))
checked(change(conn,0,owner,prop,target,8,0,None));assert not poll(conn)
checked(attrs(conn,owner,1<<11,(U*1)(1<<22)))

# Snapshot a pending selection event, its owner, a colormap, and an assigned cursor.
checked(change(conn,0,owner,prop,target,8,len(payload),c.c_char_p(payload)))
assert event(28)[16]==0
raw=response(property_reply,get_property(conn,1,owner,prop,target,0,100))
assert raw[32:32+len(payload)]==payload and event(28)[16]==1
checked(convert(conn,owner,selection,target,prop,0))
checked(change(conn,0,owner,prop,target,8,len(payload),c.c_char_p(payload)))
server_owner=bind(x,'XGetSelectionOwner',c.c_ulong,[P,c.c_ulong])(display,selection)
child=os.fork()
if child==0:
    try:
        # Fork rebuilds local HWNDs; server selection ownership remains with
        # the parent. The parent's foreign window is still a usable XID.
        assert query_owner()==server_owner
        assert response(attrs_reply,get_attrs(conn,query_owner()))[0]==1
        req=event(30);assert struct.unpack_from('<IIIII',req,8)==(requestor,owner,selection,target,prop)
        assert event(28)[16]==0
        checked(delete(conn,owner,prop));assert event(28)[16]==1
        assert struct.unpack_from('<I',response(attrs_reply,get_attrs(conn,requestor)),28)[0]==colormap
        checked(attrs(conn,requestor,1<<14,(U*1)(0)))
        checked(set_owner(conn,0,selection,0));assert query_owner()==0
        checked(free_map(conn,colormap))
    except BaseException:
        import traceback;traceback.print_exc();os._exit(1)
    os._exit(0)
assert os.waitpid(child,0)==(child,0)
# Clearing a server selection in another process is visible to this client.
assert query_owner()==0
assert struct.unpack_from('<IIIII',event(30),8)==(requestor,owner,selection,target,prop)
assert event(28)[16]==0
checked(attrs(conn,requestor,1<<14,(U*1)(0)))
checked(free_map(conn,colormap));checked(free_map(conn,colormap),12)
destroy=bind(b,'xcb_destroy_window',U,[P,U])
destroy(conn,requestor);assert query_owner()==0;destroy(conn,owner)
bind(b,'xcb_disconnect',None,[P])(conn);bind(x,'XCloseDisplay',I,[P])(display)
print('XCB_SELECTION_HANDSHAKE_CURSOR_COLORMAP_FORK_OK')
