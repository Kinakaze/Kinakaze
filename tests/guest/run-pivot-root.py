from elf_probe import run


def prepare(root):
    (root / 'newroot').mkdir(exist_ok=True)
    (root / 'newroot/value').write_bytes(b'jail\n')


run('pivot_root_probe.c', 'pivot-root-probe', 'PIVOT_ROOT_OK', prepare)
