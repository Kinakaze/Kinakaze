"""Load installed GNOME typelibs and the shared libraries they name."""
import ctypes as c
import json
from pathlib import Path


class GError(c.Structure):
    _fields_ = [('domain', c.c_uint), ('code', c.c_int), ('message', c.c_char_p)]


gi = c.CDLL('libgirepository-1.0.so.1')
gi.g_irepository_get_type.restype = c.c_ulong
gi.g_irepository_prepend_search_path.argtypes = [c.c_char_p]
gi.g_irepository_require.argtypes = [c.c_void_p, c.c_char_p, c.c_char_p, c.c_int,
                                    c.POINTER(c.POINTER(GError))]
gi.g_irepository_require.restype = c.c_void_p
gi.g_irepository_get_shared_library.argtypes = [c.c_void_p, c.c_char_p]
gi.g_irepository_get_shared_library.restype = c.c_char_p
glib = c.CDLL('libglib-2.0.so.0')
glib.g_error_free.argtypes = [c.POINTER(GError)]
gobject = c.CDLL('libgobject-2.0.so.0')
gobject.g_object_new.argtypes = [c.c_ulong, c.c_char_p]
gobject.g_object_new.restype = c.c_void_p
gobject.g_object_unref.argtypes = [c.c_void_p]
paths = sorted(Path('/usr/lib').rglob('*.typelib'))
assert paths, 'No installed typelibs'
for directory in sorted({p.parent for p in paths}):
    gi.g_irepository_prepend_search_path(str(directory).encode())

failures, loaded, libraries = [], [], {}
for path in paths:
    namespace, version = path.stem.rsplit('-', 1)
    print('Checking typelib', namespace, version, flush=True)
    # Separate metadata registries permit checking both GTK 3 and GTK 4 inputs.
    # This checks loading only, not using both toolkit versions in one app.
    repo = gobject.g_object_new(gi.g_irepository_get_type(), None)
    assert repo
    error = c.POINTER(GError)()
    metadata = gi.g_irepository_require(repo, namespace.encode(), version.encode(), 0, c.byref(error))
    if not metadata:
        message = error.contents.message.decode() if error else 'unknown typelib error'
        failures.append({'namespace': namespace, 'error': message})
        print('Typelib load failed:', message, flush=True)
        if error:
            glib.g_error_free(error)
        gobject.g_object_unref(repo)
        continue
    shared = gi.g_irepository_get_shared_library(repo, namespace.encode())
    for name in shared.decode().split(',') if shared else []:
        name = name.strip()
        if name in libraries:
            continue
        # Private namespaces can name libraries outside the default linker
        # path (GNOME Shell installs Shew under /usr/lib/gnome-shell).
        local = path.parent / name
        standard = [Path(base) / name for base in
                    ['/lib', '/usr/lib', '/lib/x86_64-linux-gnu', '/usr/lib/x86_64-linux-gnu']]
        if not local.is_file() and not any(p.is_file() for p in standard):
            candidates = sorted({p.resolve() for p in Path('/usr/lib').rglob(name) if p.is_file()})
            assert len(candidates) <= 1, (name, candidates)
            if candidates:
                local = candidates[0]
        print('Loading library', name, flush=True)
        try:
            libraries[name] = c.CDLL(str(local) if local.is_file() else name)
        except OSError as exc:
            failures.append({'namespace': namespace, 'library': name, 'error': str(exc)})
            print('Library load failed:', name, str(exc), flush=True)
    loaded.append(namespace)
    gobject.g_object_unref(repo)

print(json.dumps({'typelibs': len(loaded), 'libraries': len(libraries), 'failures': failures}), flush=True)
assert not failures, 'GNOME introspection dependencies did not all load'
print('GNOME_TYPELIB_SHARED_LIBRARY_CLOSURE_OK')
