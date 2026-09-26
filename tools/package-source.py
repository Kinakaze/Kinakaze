"""Archive a committed Kinakaze source tree and write its SHA-256 checksum."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import tomllib


ROOT = Path(__file__).resolve().parents[1]


def git(*args):
    return subprocess.check_output(['git', '-C', str(ROOT), *args])


def package(ref, tag, output):
    commit = git('rev-parse', '--verify', '--end-of-options', f'{ref}^{{commit}}').decode().strip()
    manifest = tomllib.loads(git('show', f'{commit}:Cargo.toml').decode('utf-8'))
    version = manifest['workspace']['package']['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', version):
        raise ValueError('Workspace version must be a semantic version')
    if tag:
        if tag != f'v{version}':
            raise ValueError(f'Tag {tag!r} does not match workspace version v{version}')
        tagged = git('rev-parse', '--verify', '--end-of-options', f'refs/tags/{tag}^{{commit}}').decode().strip()
        if tagged != commit:
            raise ValueError('Tag and source ref must identify the same commit')
    for name in ('README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY.md'):
        git('cat-file', '-e', f'{commit}:{name}')
    output = output.resolve()
    archive = output / f'Kinakaze-{version}-source.zip'
    checksum = archive.with_suffix('.zip.sha256')
    if archive.exists() or checksum.exists():
        raise FileExistsError('Release output already exists; choose a new output directory')
    output.mkdir(parents=True, exist_ok=True)
    # git archive includes only committed source, never local caches or credentials.
    with archive.open('xb') as stream:
        subprocess.run(['git', '-C', str(ROOT), 'archive', '--format=zip',
                        f'--prefix=Kinakaze-{version}/', commit], stdout=stream, check=True)
    with archive.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    with checksum.open('x', encoding='utf-8', newline='\n') as stream:
        stream.write(f'{digest}  {archive.name}\n')
    print(f'{archive}\n{checksum}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ref', default='HEAD', help='Committed source ref (default: HEAD)')
    parser.add_argument('--tag', help='Require this version tag to identify the source commit')
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'artifacts/releases')
    args = parser.parse_args()
    package(args.ref, args.tag, args.output_dir)


if __name__ == '__main__':
    main()
