"""Read native PE exports for build and acceptance tools, without loading code."""
from pathlib import Path
import struct


def exports(path):
    data = Path(path).read_bytes()
    def field(code, at):
        return struct.unpack_from('<' + code, data, at)[0]
    nt = field('I', 0x3c)
    if data[:2] != b'MZ' or data[nt:nt+4] != b'PE\0\0':
        raise ValueError(f'Invalid PE image: {path}')
    optional = nt + 24
    if field('H', optional) != 0x20b:
        raise ValueError(f'Expected PE32+: {path}')
    sections = []
    for index in range(field('H', nt + 6)):
        at = optional + field('H', nt + 20) + index * 40
        virtual, rva, size, raw = struct.unpack_from('<IIII', data, at + 8)
        sections.append((rva, size, raw))
    def offset(rva, length=1):
        for start, size, raw in sections:
            if start <= rva and rva - start + length <= size and raw + rva - start + length <= len(data):
                return raw + rva - start
        raise ValueError(f'Unbacked RVA in {path}: {rva:x}')
    def string(rva):
        at = offset(rva)
        end = data.index(0, at, at + 4097)
        offset(rva, end - at + 1)
        return data[at:end].decode('utf-8')
    directory = offset(field('I', optional + 112), 40)
    count = field('I', directory + 24)
    if count > 65536:
        raise ValueError(f'Excessive PE export count: {path}')
    names = offset(field('I', directory + 32), count * 4)
    return {string(field('I', names + index * 4)) for index in range(count)}


def modules(dist):
    host = Path(dist) / 'rootfs/lib'
    if not host.is_dir():
        raise FileNotFoundError(host)
    result = {}
    for path in sorted(host.iterdir()):
        if path.is_file() and (path.name.endswith('.so') or '.so.' in path.name):
            with path.open('rb') as stream:
                magic = stream.read(4)
            if magic == b'\x7fELF':
                continue
            result[path.name] = exports(path)
    return result
