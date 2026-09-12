"""Explicit observed ELF versions and their source hashes, without a catalog copy."""
import json


def read(root):
    directory = root / 'tools/abi'
    inventory = {}
    for line in (directory / 'versions.tsv').read_text(encoding='utf-8').splitlines():
        if not line or line.startswith('#'):
            continue
        fields = line.split('\t')
        if len(fields) != 3 or any(not value for value in fields):
            raise ValueError('Invalid observed ABI version row')
        soname, name, version = fields
        inventory.setdefault((soname, name), set()).add(version)
    sources = [json.loads(line) for line in (directory / 'elf-evidence.jsonl').read_text(encoding='utf-8').splitlines()]
    return inventory, sources


def outputs(root, inventory, sources):
    directory = root / 'tools/abi'
    rows = ['# soname\tsymbol\tversion']
    for (soname, name), versions in sorted(inventory.items()):
        rows.extend('\t'.join((soname, name, version)) for version in sorted(versions))
    return {
        directory / 'versions.tsv': '\n'.join(rows) + '\n',
        directory / 'elf-evidence.jsonl': ''.join(json.dumps(source, ensure_ascii=False, separators=(',', ':')) + '\n' for source in sources),
    }
