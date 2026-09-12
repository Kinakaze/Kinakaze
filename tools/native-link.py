"""Combine rustc and module .def files before ordinary native linking.

LLD accepts one definition file. Preserve rustc's Rust exports while adding the
module's C exports and import name; the resulting DLL is never patched.
"""
import ctypes
from pathlib import Path
import subprocess
import sys
import tempfile


def words(text):
    count = ctypes.c_int()
    split = ctypes.windll.shell32.CommandLineToArgvW
    split.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_int)]
    split.restype = ctypes.POINTER(ctypes.c_wchar_p)
    pointer = split("link " + text.replace("\r", " ").replace("\n", " "), ctypes.byref(count))
    if not pointer:
        raise ctypes.WinError()
    try:
        return [pointer[index] for index in range(1, count.value)]
    finally:
        ctypes.windll.kernel32.LocalFree.argtypes = [ctypes.c_void_p]
        ctypes.windll.kernel32.LocalFree(pointer)


def expand(arguments):
    result = []
    for argument in arguments:
        if argument.startswith('@'):
            data = Path(argument[1:]).read_bytes()
            result.extend(words(data.decode('utf-16' if data.startswith((b'\xff\xfe', b'\xfe\xff')) else ('utf-16-le' if b'\0' in data[:64] else 'utf-8'))))
        else:
            result.append(argument)
    return result


def main():
    arguments = expand(sys.argv[1:])
    definitions = [Path(argument[5:]) for argument in arguments if argument.lower().startswith('/def:')]
    if len(definitions) < 2:
        return subprocess.run(['lld-link', *sys.argv[1:]]).returncode
    exports = {}
    library = None
    for definition in definitions:
        for line in definition.read_text(encoding='utf-8').splitlines():
            line = line.strip()
            if not line or line.startswith(';') or line in {'EXPORTS', 'LIBRARY'}:
                continue
            if line.upper().startswith('LIBRARY '):
                if library is not None and library != line:
                    raise ValueError('Conflicting native library names')
                library = line
            else:
                name = line.split('=', 1)[0].split()[0].strip('"')
                exports[name] = line
    with tempfile.TemporaryDirectory(prefix='kinakaze-link-') as temporary:
        definition = Path(temporary) / 'exports.def'
        definition.write_text((library + '\n' if library else '') + 'EXPORTS\n' + '\n'.join(exports.values()) + '\n', encoding='utf-8')
        arguments = [arg for arg in arguments if not arg.lower().startswith('/def:')]
        arguments.append('/DEF:' + str(definition))
        # A response file avoids the Windows command-line length limit.
        response = Path(temporary) / 'link.rsp'
        response.write_text(subprocess.list2cmdline(arguments), encoding='utf-8')
        result = subprocess.run(['lld-link', '@' + str(response)])
        return result.returncode


if __name__ == '__main__':
    sys.exit(main())
