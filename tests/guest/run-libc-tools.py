"""Linux libc tools regression with a private services fixture."""
from elf_probe import run


def prepare(root):
    (root / 'etc/services').write_text('alpha 43210/tcp alpha-alias\nbeta 43211/sctp\n', encoding='utf-8', newline='\n')


if __name__ == '__main__':
    run('libc_tools_probe.c', 'libc-tools-root', 'LIBC_TOOLS_OK', prepare)
