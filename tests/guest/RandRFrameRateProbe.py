"""The current RandR mode must provide a usable compositor frame interval."""
import ctypes as c

class Mode(c.Structure):
    _fields_ = [('id', c.c_ulong), ('width', c.c_uint), ('height', c.c_uint),
                ('clock', c.c_ulong), ('hs', c.c_uint), ('he', c.c_uint),
                ('ht', c.c_uint), ('skew', c.c_uint), ('vs', c.c_uint),
                ('ve', c.c_uint), ('vt', c.c_uint), ('name', c.c_void_p),
                ('name_length', c.c_uint), ('flags', c.c_ulong)]

class Resources(c.Structure):
    _fields_ = [('timestamp', c.c_ulong), ('config_timestamp', c.c_ulong),
                ('crtc_count', c.c_int), ('crtcs', c.c_void_p),
                ('output_count', c.c_int), ('outputs', c.c_void_p),
                ('mode_count', c.c_int), ('modes', c.POINTER(Mode))]

x = c.CDLL('libX11.so.6'); r = c.CDLL('libXrandr.so.2')
x.XOpenDisplay.argtypes = [c.c_void_p]; x.XOpenDisplay.restype = c.c_void_p
r.XRRGetScreenResourcesCurrent.argtypes = [c.c_void_p, c.c_ulong]
r.XRRGetScreenResourcesCurrent.restype = c.POINTER(Resources)
r.XRRFreeScreenResources.argtypes = [c.POINTER(Resources)]
x.XCloseDisplay.argtypes = [c.c_void_p]
d = x.XOpenDisplay(None); assert d
resources = r.XRRGetScreenResourcesCurrent(d, 1); assert resources
try:
    assert resources.contents.mode_count > 0
    for mode in resources.contents.modes[:resources.contents.mode_count]:
        print(dict(width=mode.width, height=mode.height, clock=mode.clock,
                   horizontal_total=mode.ht, vertical_total=mode.vt), flush=True)
        assert mode.ht >= mode.width and mode.vt >= mode.height and mode.clock > 0
        rate = mode.clock / (mode.ht * mode.vt)
        assert 1 <= rate <= 1000, rate
        print('RANDR_COMPOSITOR_REFRESH_OK', rate, flush=True)
finally:
    r.XRRFreeScreenResources(resources); x.XCloseDisplay(d)
