"""Exercise real APT extraction and debconf preconfiguration in a disposable root."""
import argparse
import gzip
import io
import json
from pathlib import Path
import tarfile

from init_pool import InitPool


def fixture(required='1.5.0'):
    members = {
        'control': ('Package: kinakaze-debconf-probe\nVersion: 1.0\nArchitecture: all\n'
                    'Maintainer: Kinakaze test <test@example.invalid>\n'
                    f'Depends: debconf (>= {required})\nDescription: debconf integration probe\n').encode(),
        'templates': b'Template: kinakaze-debconf-probe/answer\nType: string\nDefault: pending\nDescription: Integration probe\n',
        'config': b'#!/bin/sh\nset -e\n. /usr/share/debconf/confmodule\ndb_set kinakaze-debconf-probe/answer accepted\n',
    }
    def archive(contents):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode='w') as stream:
            for name, data in contents.items():
                info = tarfile.TarInfo('./' + name)
                info.size = len(data)
                info.mode = 0o755 if name == 'config' else 0o644
                stream.addfile(info, io.BytesIO(data))
        return gzip.compress(buffer.getvalue(), mtime=0)
    output = bytearray(b'!<arch>\n')
    for name, data in [('debian-binary', b'2.0\n'), ('control.tar.gz', archive(members)), ('data.tar.gz', archive({}))]:
        output.extend(f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{100644:<8}{len(data):<10}`\n'.encode())
        output.extend(data)
        if len(data) % 2:
            output.extend(b'\n')
    return bytes(output)


SOURCE = r'''
import atexit, hashlib, json, os, pathlib, subprocess
directory = pathlib.Path('/var/tmp/debconf-preconfigure-probe')
status = pathlib.Path('/var/lib/dpkg/status')
before = status.read_bytes()
report = {'commands': [], 'passed': False}
atexit.register(lambda: (directory / 'guest-report.json').write_text(json.dumps(report, indent=2)))
def run(args, **kw):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kw)
    report['commands'].append(dict(command=args, input=kw.get('input', b'').decode(),
                                   exit=result.returncode, stdout=result.stdout.decode(), stderr=result.stderr.decode()))
    return result
archive = str(directory / 'probe.deb')
result = run(['/usr/bin/apt-extracttemplates', '--tempdir', str(directory), archive])
assert result.returncode == 0, result.stderr
fields = result.stdout.decode().split()
assert fields[0] == 'kinakaze-debconf-probe' and len(fields) == 3, result.stdout
template, config = map(pathlib.Path, fields[-2:])
assert b'Template: kinakaze-debconf-probe/answer' in template.read_bytes()
assert b'db_set kinakaze-debconf-probe/answer accepted' in config.read_bytes()
template.unlink(); config.unlink()
unsupported = run(['/usr/bin/apt-extracttemplates', str(directory / 'future.deb')])
assert unsupported.returncode == 0 and not unsupported.stdout, unsupported
custom = directory / 'custom status'
custom.write_bytes(before.rstrip() + b'\n\nPackage: debconf\nStatus: install ok installed\nArchitecture: all\nVersion: 1.5.99\nDescription: explicit test version\n\n')
actual = run(['/usr/bin/apt-extracttemplates', '-o', 'Dir::State::status=' + str(custom),
              '--tempdir', str(directory), str(directory / 'newer.deb')])
assert actual.returncode == 0 and actual.stdout.startswith(b'kinakaze-debconf-probe '), actual.stderr
for name in actual.stdout.decode().split()[-2:]:
    pathlib.Path(name).unlink()
custom.unlink()
environment = dict(os.environ, DEBIAN_FRONTEND='noninteractive', DEBCONF_NONINTERACTIVE_SEEN='true')
configured = run(['/usr/sbin/dpkg-preconfigure', archive], env=environment)
assert configured.returncode == 0, configured.stderr
assert b'apt-extracttemplates failed' not in configured.stderr, configured.stderr
assert b'Cannot get debconf version' not in configured.stderr, configured.stderr
hook = run(['/usr/sbin/dpkg-preconfigure', '--apt'], input=(archive + '\n').encode(), env=environment)
assert hook.returncode == 0 and b'apt-extracttemplates failed' not in hook.stderr, hook.stderr
answer = run(['/usr/bin/debconf-communicate', 'kinakaze-debconf-probe'], input=b'GET kinakaze-debconf-probe/answer\n', env=environment)
assert answer.returncode == 0 and answer.stdout == b'0 accepted\n', answer.stdout
purge = run(['/usr/bin/debconf-communicate', 'kinakaze-debconf-probe'], input=b'PURGE\n', env=environment)
assert purge.returncode == 0 and purge.stdout.startswith(b'0'), purge.stdout
invalid = run(['/usr/bin/apt-extracttemplates', str(directory / 'invalid.deb')])
assert invalid.returncode != 0 and invalid.stderr, invalid
assert status.read_bytes() == before, 'real dpkg status was changed'
report['status_sha256'] = hashlib.sha256(before).hexdigest()
installed = run(['/usr/bin/apt-get', '-y', 'install', archive], env=environment)
assert installed.returncode == 0, installed.stderr
assert b'apt-extracttemplates failed' not in installed.stderr, installed.stderr
assert b'Cannot get debconf version' not in installed.stderr, installed.stderr
assert b'Package: debconf\n' not in status.read_bytes(), 'component became a real dpkg package'
assert b'Package: kinakaze-debconf-probe\n' in status.read_bytes()
removed = run(['/usr/bin/dpkg', '--purge', 'kinakaze-debconf-probe'], env=environment)
assert removed.returncode == 0, removed.stderr
purged = run(['/usr/bin/debconf-communicate', 'kinakaze-debconf-probe'], input=b'PURGE\n', env=environment)
assert purged.returncode == 0, purged.stderr
report['passed'] = True
(directory / 'guest-report.json').write_text(json.dumps(report, indent=2))
print('DEBCONF_PRECONFIGURE_OK', flush=True)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--reuse-fixture', action='store_true')
    args = parser.parse_args()
    root, dist = args.root.resolve(), args.dist.resolve()
    directory = root / 'var/tmp/debconf-preconfigure-probe'
    directory.mkdir(parents=True, exist_ok=args.reuse_fixture)
    (directory / 'probe.deb').write_bytes(fixture())
    (directory / 'future.deb').write_bytes(fixture('999'))
    (directory / 'newer.deb').write_bytes(fixture('1.5.90'))
    (directory / 'invalid.deb').write_bytes(b'invalid archive')
    with InitPool(root, dist, args.output / 'session', size=2, timeout=180) as pool:
        result = pool.run(['/usr/bin/python3.11', '-c', SOURCE], expect=['DEBCONF_PRECONFIGURE_OK'],
                          environment=['PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL=C'])
    if (directory / 'guest-report.json').exists():
        result['guest'] = json.loads((directory / 'guest-report.json').read_text(encoding='utf-8'))
    (args.output / 'report.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(dict(status=result['status'], exit=result['exit_code'])))
    return int(result['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
