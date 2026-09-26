"""Exercise offline setup, login shell and real APT dependency transactions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from contextlib import contextmanager


@contextmanager
def temporary_root():
    temporary = tempfile.TemporaryDirectory(prefix='kinakaze apt ')
    try:
        yield temporary.name
    finally:
        # Windows can briefly retain delete-pending mmap temporary files after
        # the supervisor exits. Retry only cleanup of this owned test directory.
        deadline = time.monotonic() + 5
        while True:
            try:
                temporary.cleanup()
                break
            except PermissionError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--network', action='store_true', help='also update signed Debian sources and exercise bzip2')
    parser.add_argument('--proxy', help='HTTP proxy for the optional network test only')
    args = parser.parse_args()
    dist = args.dist.resolve()
    checks = []
    with temporary_root() as directory:
        root = Path(directory) / 'root with spaces'
        env = {k: v for k, v in os.environ.items() if not k.startswith('KINAKAZE_')}
        env['PATH'] = ''

        def invoke(arguments, input=None, expected=0, timeout=90):
            result = subprocess.run([str(dist / 'worker.exe'), *arguments], input=None if input is None else input.encode(), env=env,
                                    capture_output=True,
                                    timeout=timeout, creationflags=subprocess.CREATE_NO_WINDOW)
            if result.returncode != expected:
                raise AssertionError(f'{arguments}: exit {result.returncode}\n{result.stdout.decode(errors="replace")}\n{result.stderr.decode(errors="replace")}')
            return result.stdout.decode('utf-8', errors='replace')

        def shell(script, **kwargs):
            return invoke(['--root', str(root), '--', '/bin/sh', '-lc', 'set -eu\n' + script], **kwargs)

        invoke(['setup', '--root', str(root)])
        assert not any(root.glob('**/.kinakaze-aot*'))
        assert (root / 'usr/bin/apt-get').is_file()
        checks.append('offline setup creates complete APT root without starting a guest')
        assert shell('stat -c "%a %u:%g" /usr/share/keyrings/debian-archive-keyring.gpg /tmp').splitlines() == ['644 0:0', '1777 0:0']
        checks.append('offline inode permissions survive guest startup and permit the APT sandbox')
        assert invoke(['--root', str(root)], input='command -v apt-get\nexit 17\n', expected=17).strip() == '/usr/bin/apt-get'
        shell('apt-get check; dpkg --audit; dpkg-query -S /usr/bin/apt-get')
        checks.append('default login shell supplies PATH and propagates exit status; dpkg owns the base')
        repo = root / 'tmp/repo'
        repo.mkdir()
        controls = {}
        for name in ('kinakaze-probe-lib', 'kinakaze-probe-app'):
            package = repo / name
            (package / 'DEBIAN').mkdir(parents=True)
            dependency = 'Depends: kinakaze-probe-lib (= 1)\n' if name.endswith('-app') else ''
            control = f'Package: {name}\nVersion: 1\nArchitecture: amd64\nMaintainer: Test <test@example.invalid>\n{dependency}Description: First-run transaction probe\n'
            controls[name] = control
            (package / 'DEBIAN/control').write_text(control, encoding='utf-8', newline='\n')
            (package / 'DEBIAN/postinst').write_text(f'#!/bin/sh\nset -e\nprintf configured > /tmp/{name}.configured\n', encoding='utf-8', newline='\n')
            payload = package / 'usr/share/kinakaze-probe' / name
            payload.parent.mkdir(parents=True)
            payload.write_text(name, encoding='utf-8', newline='\n')
            shell(f'chmod 755 /tmp/repo/{name}/DEBIAN /tmp/repo/{name}/DEBIAN/postinst\n'
                  f'find /tmp/repo/{name}/usr -type d -exec chmod 755 {{}} +\n'
                  f'chmod 644 /tmp/repo/{name}/usr/share/kinakaze-probe/{name}\n'
                  f'chmod 644 /tmp/repo/{name}/DEBIAN/control\n'
                  f'dpkg-deb --build /tmp/repo/{name} /tmp/repo/{name}.deb')
        index = ''
        for name, control in controls.items():
            data = (repo / f'{name}.deb').read_bytes()
            index += control + f'Filename: ./{name}.deb\nSize: {len(data)}\nSHA256: {hashlib.sha256(data).hexdigest()}\n\n'
        (repo / 'Packages').write_text(index, encoding='utf-8', newline='\n')
        # This isolated fixture is intentionally unsigned. The shipped Debian
        # sources keep their Signed-By keyring and normal signature validation.
        (root / 'tmp/probe.list').write_text('deb [trusted=yes] file:/tmp/repo ./\n', encoding='utf-8', newline='\n')
        apt = 'apt-get -o Dir::Etc::sourcelist=/tmp/probe.list -o Dir::Etc::sourceparts=-'
        shell(f'{apt} -o APT::Update::Error-Mode=any update\n{apt} -y install kinakaze-probe-app', timeout=180)
        for name in controls:
            assert (root / f'usr/share/kinakaze-probe/{name}').read_text() == name
            assert (root / f'tmp/{name}.configured').read_text() == 'configured'
        shell(f'{apt} -y purge kinakaze-probe-app kinakaze-probe-lib\ndpkg --audit')
        assert not any((root / f'usr/share/kinakaze-probe/{name}').exists() for name in controls)
        checks.append('APT local index, dependency resolution, dpkg install, maintainer scripts and purge')
        if args.network:
            if args.proxy:
                (root / 'etc/apt/apt.conf.d/99test-proxy').write_text(
                    f'Acquire::https::Proxy {json.dumps(args.proxy)};\n', encoding='utf-8', newline='\n')
            # bzip2 is part of Debian's standard preset; preserve that base tool.
            has_bzip2 = (root / 'bin/bzip2').is_file()
            shell('apt-get -o APT::Update::Error-Mode=any update\napt-get -y install hello\nhello\n'
                  + ('' if has_bzip2 else 'apt-get -y install bzip2\n') +
                  'printf package-roundtrip > /tmp/roundtrip\nbzip2 -k /tmp/roundtrip\n'
                  'bzip2 -dc /tmp/roundtrip.bz2 > /tmp/restored\ncmp /tmp/roundtrip /tmp/restored\n'
                  'apt-get -y purge hello\ntest ! -e /usr/bin/hello\n'
                  + ('' if has_bzip2 else 'apt-get -y purge bzip2\ntest ! -e /bin/bzip2\n') + 'dpkg --audit', timeout=600)
            checks.append('signed HTTPS Debian index, hello execution, bzip2 compression roundtrip and removal')
        (root / 'etc/hostname').write_text('preserved\n', encoding='utf-8', newline='\n')
        invoke(['setup', '--root', str(root), '--rootfs-manifest', str(root / 'missing.json')])
        assert (root / 'etc/hostname').read_text() == 'preserved\n'
        checks.append('repeated setup preserves a nonempty root without opening a manifest')
    report = dict(passed=True, checks=checks, images={name: hashlib.sha256((dist / name).read_bytes()).hexdigest()
                  for name in ('init.exe', 'worker.exe', 'rootfs.manifest.json', 'kinakaze.cmd')})
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8', newline='\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
