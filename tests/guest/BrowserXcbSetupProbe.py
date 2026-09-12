"""Decode xcb_get_setup exactly as Chromium's independent X11 parser does."""
import ctypes as c
import struct

lib=c.CDLL('libxcb.so.1')
class Iterator(c.Structure):
    _fields_=[('data',c.c_void_p),('rem',c.c_int),('index',c.c_int)]
for name,result,args in (
    ('xcb_connect',c.c_void_p,[c.c_char_p,c.c_void_p]),
    ('xcb_disconnect',None,[c.c_void_p]),
    ('xcb_get_setup',c.c_void_p,[c.c_void_p]),
    ('xcb_setup_pixmap_formats',c.c_void_p,[c.c_void_p]),
    ('xcb_setup_pixmap_formats_iterator',Iterator,[c.c_void_p]),
    ('xcb_format_next',None,[c.POINTER(Iterator)]),
    ('xcb_setup_roots_iterator',Iterator,[c.c_void_p]),
    ('xcb_screen_allowed_depths_iterator',Iterator,[c.c_void_p]),
    ('xcb_depth_visuals_iterator',Iterator,[c.c_void_p]),
    ('xcb_depth_next',None,[c.POINTER(Iterator)]),
    ('xcb_visualtype_next',None,[c.POINTER(Iterator)]),
    ('xcb_screen_next',None,[c.POINTER(Iterator)]),
):
    fn=getattr(lib,name);fn.restype=result;fn.argtypes=args
connection=lib.xcb_connect(None,None)
assert connection
try:
    base=lib.xcb_get_setup(connection)
    header=c.string_at(base,40)
    assert header[0]==1 and struct.unpack_from('<H',header,2)[0]==11
    size=8+struct.unpack_from('<H',header,6)[0]*4
    assert 40<=size<=65536
    data=c.string_at(base,size)
    vendor_size=struct.unpack_from('<H',header,24)[0]
    at=40+(vendor_size+3)//4*4
    assert at<=size and data[40:40+vendor_size].isascii()
    assert lib.xcb_setup_pixmap_formats(base)==base+at
    format_iterator=lib.xcb_setup_pixmap_formats_iterator(base)
    assert format_iterator.rem==header[29] and format_iterator.index==at
    for index in range(header[29]):
        assert format_iterator.data==base+at+index*8
        lib.xcb_format_next(c.byref(format_iterator))
    assert format_iterator.data==base+at+header[29]*8
    formats={data[at+i*8]:data[at+i*8+1] for i in range(header[29])}
    at+=header[29]*8
    screens=lib.xcb_setup_roots_iterator(base)
    assert screens.rem==header[28] and screens.index==at
    for _ in range(screens.rem):
        assert screens.data==base+at and at+40<=size
        screen=data[at:at+40];root_visual=struct.unpack_from('<I',screen,32)[0]
        depths=lib.xcb_screen_allowed_depths_iterator(base+at)
        assert depths.rem==screen[39] and depths.index==40
        at+=40;found=False
        for _ in range(depths.rem):
            assert depths.data==base+at and at+8<=size
            depth=data[at];count=struct.unpack_from('<H',data,at+2)[0]
            visuals=lib.xcb_depth_visuals_iterator(base+at)
            assert visuals.rem==count and visuals.data==base+at+8
            at+=8
            for _ in range(count):
                assert visuals.data==base+at and at+24<=size
                visual=struct.unpack_from('<I',data,at)[0]
                found|=depth==screen[38] and visual==root_visual
                at+=24;lib.xcb_visualtype_next(c.byref(visuals))
            lib.xcb_depth_next(c.byref(depths))
        assert found and screen[38] in formats, 'root visual missing from wire setup'
        lib.xcb_screen_next(c.byref(screens))
        assert screens.data==base+at and screens.index==at
    assert at==size, ('setup length',at,size)
    # Accessors also accept caller-owned copies, including empty format lists.
    copied=c.create_string_buffer(data)
    copied[29]=b'\0'
    empty=lib.xcb_setup_pixmap_formats_iterator(c.addressof(copied))
    assert empty.rem==0 and empty.data==c.addressof(copied)+40+(vendor_size+3)//4*4
finally:lib.xcb_disconnect(connection)
print('BROWSER_XCB_WIRE_SETUP_OK',flush=True)
