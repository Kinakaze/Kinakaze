from elf_probe import run


def prepare(root):
    (root / 'newroot').mkdir(exist_ok=True)
    (root / 'outside').mkdir(exist_ok=True)
    (root / 'newroot/value').write_bytes(b'jail\n')


run('setns_root_probe.c', 'setns-root-probe', 'SETNS_ROOT_FORK_OK', prepare)
