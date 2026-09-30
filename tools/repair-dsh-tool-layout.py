"""Unify identical DSH scheduler modules in a staged guest installation.

Moves duplicate packages to a guest backup directory and links them to the
canonical package. Package source is unchanged; differing releases are refused.
"""
import argparse
import hashlib
import json
from pathlib import Path
import uuid

from init_pool import InitPool


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--install', required=True, help='absolute guest dsh installation directory')
    args = parser.parse_args()
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    install = (root / args.install.lstrip('/')).resolve()
    if not args.install.startswith('/') or not install.is_relative_to(root) or install == root:
        parser.error('--install must stay inside the guest root')
    canonical = install / 'node_modules/@deepseek-ai/dsh-base/node_modules/@deepseek-ai/dsh-tools'

    def digest(package):
        files = sorted(p for p in package.rglob('*') if p.is_file()
                       and 'node_modules' not in p.relative_to(package).parts)
        return {p.relative_to(package).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in files}

    expected = digest(canonical)
    if not expected or 'lib/index.js' not in expected:
        parser.error('canonical dsh-tools package is missing')
    packages = sorted({p.parent.resolve() for p in install.rglob('dsh-tools/package.json')})
    duplicates = []
    for package in packages:
        if not package.is_relative_to(install):
            parser.error('package escapes the staged installation')
        if digest(package) != expected:
            parser.error('refusing to unify different dsh-tools packages: ' + str(package))
        if package != canonical:
            duplicates.append('/' + package.relative_to(root).as_posix())
    output.mkdir(parents=True, exist_ok=False)
    guest_canonical = '/' + canonical.relative_to(root).as_posix()
    backup = '/tmp/dsh-module-backup-' + uuid.uuid4().hex
    plan = dict(install=args.install, canonical=guest_canonical, duplicates=duplicates,
                backup=backup, canonical_files=expected)
    (output / 'plan.json').write_text(json.dumps(plan, indent=2), encoding='utf-8')
    source = '''import json, os, sys
plan = json.loads(sys.argv[1])
install = os.path.realpath(plan['install'])
canonical = os.path.realpath(plan['canonical'])
assert os.path.commonpath([install, canonical]) == install
assert plan['backup'].startswith('/tmp/dsh-module-backup-')
os.mkdir(plan['backup'])
moved = []
try:
    for index, source in enumerate(plan['duplicates']):
        assert os.path.commonpath([install, os.path.realpath(source)]) == install
        assert os.path.realpath(source) != canonical and not os.path.islink(source)
        target = os.path.join(plan['backup'], str(index))
        os.rename(source, target)
        moved.append((source, target))
        os.symlink(canonical, source)
        assert os.path.realpath(source) == canonical
except BaseException:
    for source, target in reversed(moved):
        if os.path.islink(source): os.unlink(source)
        os.rename(target, source)
    raise
print('DSH_TOOL_MODULES_UNIFIED', len(moved), flush=True)
'''
    with InitPool(root, dist, output / 'session', timeout=30, size=1,
                  memory_limit_bytes=1024**3) as pool:
        result = pool.run(['/usr/bin/python3', '-c', source, json.dumps(plan)],
                          expect=['DSH_TOOL_MODULES_UNIFIED'])
    (output / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(dict(status=result['status'], unified=len(duplicates), backup=backup)), flush=True)
    return int(result['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
