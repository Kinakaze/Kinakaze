from elf_probe import run


def prepare(root):
    fixtures = {
        'hosts': '# skip\nbad ignored\n127.0.0.1 localhost loopback\n::1 ipv6-localhost ip6\n',
        'protocols': '# skip\ntcp 6 TCP\nudp 17 UDP\n',
        'services': 'http 80/tcp www\ndomain 53/udp dns\n',
        'networks': 'loopback 127.0.0.0 loop\nprivate 10.0.0.0 lan\n',
        'rpc': 'portmapper 100000 portmap sunrpc\nnfs 100003 nfsprog\n',
        'passwd': 'bad:x:invalid:0:Bad:/bad:/bin/sh\nroot:x:0:0:root:/root:/bin/bash\nguest:x:1000:1000:Guest:/home/guest:/bin/sh\n',
        'group': 'bad:x:invalid:root\nroot:x:0:root\nguest:x:1000:guest\n',
    }
    for name, text in fixtures.items():
        (root / 'etc' / name).write_text(text, encoding='utf-8', newline='\n')


run('StandardAbiProbe.c', 'standard-abi-probe', 'STANDARD_ABI_OK', prepare=prepare, libraries=('libm.so.6',))
