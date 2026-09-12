"""Read Cargo and linker inputs directly; no module configuration files."""
import re
import tomllib


def read_cargo(directory):
    return tomllib.loads((directory / 'Cargo.toml').read_text(encoding='utf-8'))


def soname(directory):
    definition = (directory / 'exports.def').read_text(encoding='utf-8')
    match = re.search(r'^LIBRARY\s+"?([^"\s]+)"?$', definition, re.MULTILINE)
    if not match:
        raise ValueError(f'Missing linker library name: {directory}')
    return match[1]


def load_inputs(root):
    directories = [root / 'crates/runtime', *sorted(path.parent for path in (root / 'libs').glob('*/Cargo.toml'))]
    inputs = {}
    owners = {}
    for directory in [*directories, *(root / 'engine/crates').iterdir()]:
        if not (directory / 'exports.def').is_file():
            continue
        cargo = read_cargo(directory)
        if 'dylib' in cargo.get('lib', {}).get('crate-type', []):
            name = soname(directory)
            if name in owners:
                raise ValueError(f'Duplicate native import name: {name}')
            owners[name] = directory.name
    for directory in directories:
        cargo = read_cargo(directory)
        native = 'dylib' in cargo['lib']['crate-type']
        if not native and 'cdylib' not in cargo['lib']['crate-type']:
            raise ValueError(f'Missing native library output: {directory}')
        if not (directory / 'src/lib.rs').is_file():
            raise ValueError(f'Missing module source: {directory}')
        if (directory / 'implementation').exists() or 'implementation' in cargo.get('dependencies', {}):
            raise ValueError(f'Implementation must live directly in {directory}/src')
        owner = directory.name
        if not native:
            definition = (directory / 'exports.def').read_text(encoding='utf-8')
            referenced = {line.split('=', 1)[1].split()[0].rsplit('.', 1)[0]
                          for line in definition.splitlines() if '=' in line}
            implementations = {owners[name] for name in referenced}
            if len(implementations) != 1:
                raise ValueError(f'Expected one implementation owner: {directory}')
            owner = implementations.pop()
        inputs[directory.name] = dict(directory=directory, library=cargo['lib']['name'],
                                      soname=soname(directory), native=native, implementation=owner)
    return inputs


def source_directory(root, name):
    directories = [directory for directory in (root / 'libs' / name, root / 'engine/crates' / name)
                   if (directory / 'Cargo.toml').is_file()]
    if len(directories) != 1:
        raise ValueError(f'Expected one source directory for {name}: {directories}')
    return directories[0]
