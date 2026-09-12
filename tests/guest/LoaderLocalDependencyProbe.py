"""A global dlopen must relocate against an already loaded local dependency."""
import ctypes
import os

font = ctypes.CDLL('libfreetype.so.6', mode=os.RTLD_LOCAL | os.RTLD_NOW)
assert font.FT_Set_Var_Design_Coordinates
pango = ctypes.CDLL('libpango-1.0.so.0', mode=os.RTLD_GLOBAL | os.RTLD_NOW)
assert pango.pango_font_description_new
print('DLOPEN_GLOBAL_REUSES_LOCAL_DEPENDENCY_OK', flush=True)
