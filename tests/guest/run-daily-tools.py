from elf_probe import run


def prepare(root):
    fixtures = {
        'aliases': '# comment\nteam: alice, bob,\n  carol # continuation\nroot: admin\n',
        'gshadow': 'bad\nteam:!:alice:bob,carol\nempty:!::\n',
        'hosts': '127.0.0.1 fixture-host alias-host\n',
        'ethers': '# comment\ninvalid row\n02:00:00:00:00:ff printer\n',
    }
    for name, text in fixtures.items():
        (root / 'etc' / name).write_text(text, encoding='utf-8', newline='\n')


run('DailyToolsProbe.c', 'daily-tools-probe', 'DAILY_TOOLS_OK', prepare=prepare)
