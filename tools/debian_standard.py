"""Read every payload of the locked standard installation, without host extraction."""
from concurrent.futures import ThreadPoolExecutor
from email.parser import Parser
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import posixpath
import sys
import tarfile
import time
import urllib.parse
import urllib.request

sys.path.insert(0, str(Path(__file__).with_name('guest-deps')))
from guest_deps import ar_members, atomic_write, archive_path, elf_info, OFFICIAL_HOSTS


def stored_path(name):
    """Use the VFS's Interix encoding for Linux names reserved by Windows."""
    name = archive_path(name)
    return '/'.join(''.join(chr(0xf000 + ord(c)) if c in '<>:"|?*\\' or ord(c) < 32
                           or (i == len(part) - 1 and c in '. ') else c
                           for i, c in enumerate(part)) for part in name.split('/'))


def fetch(package, cache, offline=False):
    path = Path(cache) / package['filename']
    def verified(data):
        if len(data) != package['size'] or hashlib.sha256(data).hexdigest() != package['sha256']:
            raise ValueError('Debian archive SHA-256 mismatch: ' + package['package'])
        return data
    if path.is_file():
        verified(path.read_bytes())
        return path
    if offline:
        raise FileNotFoundError('offline Debian archive missing: ' + str(path))
    uri = urllib.parse.urlparse(package['url'])
    if uri.scheme != 'https' or uri.hostname not in OFFICIAL_HOSTS:
        raise ValueError('Debian archive must use an official HTTPS origin')
    for attempt in range(4):
        try:
            with urllib.request.urlopen(package['url'], timeout=45) as response:
                final = urllib.parse.urlparse(response.url)
                if final.scheme != 'https' or final.hostname not in OFFICIAL_HOSTS:
                    raise ValueError('Debian archive redirected outside official HTTPS origins')
                data = verified(response.read(package['size'] + 1))
            atomic_write(path, data)
            return path
        except (OSError, TimeoutError):
            if attempt == 3:
                raise
            time.sleep(attempt + 1)


def load(lock_path, cache, provided, offline=False, sources=None):
    lock = json.loads(Path(lock_path).read_text(encoding='utf-8'))
    if lock.get('schema') != 1 or lock.get('architecture') != 'amd64':
        raise ValueError('unsupported Debian installation lock')
    packages = lock['packages']
    names = {p['package'] for p in packages}
    if len(names) != len(packages) or not set(lock['selection']['packages']).issubset(names):
        raise ValueError('incomplete/duplicate Debian standard package set')
    with ThreadPoolExecutor(max_workers=4) as pool:
        paths = list(pool.map(lambda p: fetch(p, cache, offline), packages))
    files, links, modes, directories, owners, adaptations = {}, {}, {}, set(), {}, []
    hardlinks = {}
    for package, path in zip(packages, paths):
        archive = ar_members(path.read_bytes())
        # Preserve virtual dependencies (for example perlapi-5.36.0) from the
        # authenticated control archive as well as the concrete package name.
        control = next(data for name, data in archive.items() if name.startswith('control.tar'))
        with tarfile.open(fileobj=io.BytesIO(control), mode='r:*') as tar:
            member = next(member for member in tar if member.name in ('control', './control'))
            fields = Parser().parsestr(tar.extractfile(member).read().decode('utf-8'))
            if package['package'] == 'ucf':
                templates = next(member for member in tar.getmembers()
                                 if member.name in ('templates', './templates'))
                files['usr/share/kinakaze/ucf.templates'] = tar.extractfile(templates).read()
        package['provides'] = ' '.join(fields.get('Provides', '').split())
        payload = next(data for name, data in archive.items() if name.startswith('data.tar.'))
        with tarfile.open(fileobj=io.BytesIO(payload), mode='r:*') as tar:
            for member in tar:
                if member.name in ('.', './'):
                    continue
                name = archive_path(member.name)
                # Native modules own these SONAMEs; upstream providers would
                # introduce a second libc/loader ABI into the hosted process.
                if PurePosixPath(name).name in provided and '/doc/' not in name:
                    adaptations.append(dict(package=package['package'], path=name, reason='native ABI provider'))
                    continue
                if member.isdir():
                    directories.add(name)
                    modes[name] = member.mode & 0o7777
                    continue
                if name in owners:
                    raise ValueError(f'overlapping Debian package payload: {name}: {owners[name]} / {package["package"]}')
                owners[name] = package['package']
                modes[name] = member.mode & 0o7777
                if member.isfile():
                    data = tar.extractfile(member).read()
                    if data.startswith(b'\x7fELF') and elf_info(data).soname in provided:
                        adaptations.append(dict(package=package['package'], path=name,
                                                provider=elf_info(data).soname, reason='native ABI provider'))
                    else:
                        files[name] = data
                        if sources is not None:
                            sources[name] = dict(archive=package['package'], member=member.name,
                                                 sha256=hashlib.sha256(data).hexdigest())
                elif member.issym():
                    links[name] = member.linkname
                elif member.islnk():
                    hardlinks[name] = archive_path(member.linkname)
                else:
                    raise ValueError('unsupported Debian archive node: ' + name)
    for name, target in hardlinks.items():
        seen = {name}
        while target in hardlinks:
            if target in seen:
                raise ValueError('Debian hardlink cycle')
            seen.add(target)
            target = hardlinks[target]
        files[name] = files[target]
        if sources is not None:
            sources[name] = sources[target]
    return files, links, modes, directories, packages, adaptations
