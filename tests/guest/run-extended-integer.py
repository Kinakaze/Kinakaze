from elf_probe import run

run('extended_integer_probe.c', 'extended-integer-probe', 'EXTENDED_INTEGER_OK', libraries=('libm.so.6',))
