from pathlib import Path
from elf_probe import checked, run


def prepare(root):
    source = Path(__file__).with_name('loader_entry_dso.c')
    checked(['clang', '--target=x86_64-linux-gnu', '-ffreestanding', '-fno-stack-protector',
             '-fno-builtin', '-fPIC', '-O1', '-c', source, '-o', root / 'dso.o'])
    checked(['ld.lld', '-shared', '-soname', 'loader-entry-dso.so', root / 'dso.o',
             '-o', root / 'loader-entry-dso.so'])


run('loader_entry_probe.c', 'loader-entry-probe', 'LOADER_ENTRY_OK',
    prepare=prepare,
    extra_files={'libdl': 'rootfs/lib/libdl.so.2', 'loader': 'rootfs/lib/ld-linux-x86-64.so.2'},
    libraries=('libdl.so.2',))
