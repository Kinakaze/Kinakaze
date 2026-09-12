from elf_probe import run


def prepare(root):
    (root / 'etc/services').unlink(missing_ok=True)


run('services_probe.c', 'services-probe', 'SERVICES_DB_OK', prepare)
