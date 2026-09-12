from elf_probe import run

def prepare(root):
    (root / 'etc/netgroup').write_text('inner (host,alice,fixture)\nouter inner\ncycle cycle\n', encoding='utf-8')
    directory = root / 'tmp/glob'
    directory.mkdir(exist_ok=True)
    for name in ('a.txt', 'b.txt', '.hidden.txt', 'c.dat'):
        (directory / name).write_bytes(b'fixture')

run('libc_gap_probe.c', 'libc-gap-probe', 'LIBC_GAP_OK', prepare=prepare,
    cflags=('-O2', '-fomit-frame-pointer', '-fasynchronous-unwind-tables'),
    ldflags=('--eh-frame-hdr', '--export-dynamic'),
    extra_files={'libc': 'rootfs/lib/libc.so.6', 'loader': 'rootfs/lib/ld-linux-x86-64.so.2'})
