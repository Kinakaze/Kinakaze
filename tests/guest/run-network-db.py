"""Guest network/protocol files, MAC ABI, TLS ownership and native fork."""
from elf_probe import run


def prepare(root):
    (root / 'etc/protocols').write_bytes(b'# fixture\nbad xyz\ntcp 6 TCP\nsctp 132 SCTP\n')
    (root / 'etc/networks').write_bytes(b'# fixture\nbad 1.256\nTestNet 10 alias\nprivate 192.168\n')


if __name__ == '__main__':
    run('network_db_probe.c', 'network-db-root', 'NETWORK_DB_OK', prepare)
