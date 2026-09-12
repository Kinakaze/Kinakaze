"""Direct PE math, shared errno, symbol versions and native fork restoration."""
from elf_probe import run

if __name__ == '__main__':
    run('native_math_probe.c', 'native-math-root', 'NATIVE_MATH_OK',
        extra_files={'math': 'rootfs/lib/libm.so.6'}, libraries=('libdl.so.2',))
