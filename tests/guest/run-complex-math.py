"""Versioned libm imports, complex calling conventions and binary80 numerics."""
from elf_probe import run

if __name__ == '__main__':
    run('ComplexMathProbe.c', 'complex-math-root', 'COMPLEX_MATH_OK',
        extra_files={'math': 'rootfs/lib/libm.so.6'},
        libraries=('libm.so.6', 'libdl.so.2'), worker_mode='oneshot')
