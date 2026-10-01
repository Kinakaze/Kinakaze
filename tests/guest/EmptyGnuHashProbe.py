import pathlib
import struct
import subprocess
import tempfile


with tempfile.TemporaryDirectory(prefix='gnu-hash-') as directory:
    root = pathlib.Path(directory)
    library = root / 'library.c'
    library.write_text('int imported_value(void) { return 42; }\n')
    subprocess.run(['/usr/bin/gcc', '-nostdlib', '-shared', '-fPIC', str(library),
                    '-o', str(root / 'libprobe.so')], check=True, timeout=60)
    source = root / 'main.c'
    source.write_text('''extern int imported_value(void);
extern int optional_value(void) __attribute__((weak));
void _start(void) {
    int status = imported_value() != 42 || optional_value != 0;
    __asm__ volatile("syscall" : : "a"(60L), "D"((long)status) : "rcx", "r11", "memory");
    __builtin_unreachable();
}
''')
    binary = root / 'empty-gnu-hash'
    subprocess.run(['/usr/bin/gcc', '-nostdlib', '-no-pie', '-Wl,--hash-style=gnu',
                    '-Wl,-rpath,$ORIGIN', str(source), '-L' + str(root), '-lprobe',
                    '-o', str(binary)], check=True, timeout=60)
    image = bytearray(binary.read_bytes())
    struct.pack_into('<Q', image, 40, 0)
    struct.pack_into('<HHH', image, 58, 0, 0, 0)
    binary.write_bytes(image)
    result = subprocess.run([str(binary)], capture_output=True, timeout=15)
    assert result.returncode == 0, (result.returncode, result.stderr)
print('EMPTY_GNU_HASH_IMPORTS_OK')
