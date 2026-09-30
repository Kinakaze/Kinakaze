"""Time real dpkg unpack/configure with private state and checked payloads.

No global apt/dpkg state or sync policy is modified. All temporary writes live
below a unique directory in the supplied guest root's var/tmp.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import shutil
import statistics
import tarfile
import uuid

from init_pool import InitPool, distribution_hashes


def tar(entries):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode='w', format=tarfile.GNU_FORMAT) as archive:
        for name, data, mode in entries:
            info = tarfile.TarInfo('./' + name)
            info.mode, info.mtime = mode, 1700000000
            if data is None:
                info.type = tarfile.DIRTYPE
                archive.addfile(info)
            else:
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
    return gzip.compress(stream.getvalue(), mtime=0)


def package(files, children):
    payload = [(name, None, 0o755) for name in
               ('usr', 'usr/share', 'usr/share/kinakaze-dpkg-probe')]
    for index in range(files):
        data = hashlib.sha256(str(index).encode()).digest() * 128
        payload.append((f'usr/share/kinakaze-dpkg-probe/file-{index}', data, 0o644))
    control = ('Package: kinakaze-dpkg-probe\nVersion: 1.0\nArchitecture: amd64\n'
               'Maintainer: Probe <probe@example.invalid>\nDescription: Isolated fixture\n')
    script = ('#!/bin/sh\nset -eu\n' + '/bin/true\n' * children
              + 'printf configured > "$DPKG_ROOT/configured"\n')
    members = [('debian-binary', b'2.0\n'), ('control.tar.gz', tar([
        ('control', control.encode(), 0o644), ('postinst', script.encode(), 0o755)])),
        ('data.tar.gz', tar(payload))]
    result = bytearray(b'!<arch>\n')
    for name, data in members:
        result.extend(f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(data):<10}`\n'.encode())
        result.extend(data)
        if len(data) % 2:
            result.extend(b'\n')
    return bytes(result), payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--files', type=int, default=256)
    parser.add_argument('--children', type=int, default=20)
    parser.add_argument('--timeout', type=float, default=600)
    parser.add_argument('--keep', action='store_true')
    parser.add_argument('--io-probe', action='store_true')
    parser.add_argument('--apt', action='store_true', help='also install the local package through apt-get')
    args = parser.parse_args()
    if min(args.repeat, args.files, args.timeout) <= 0 or args.children < 0:
        parser.error('counts/timeouts must be positive; children must be nonnegative')
    root, dist, output = args.root.resolve(), args.dist.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    guest = '/var/tmp/dpkg-phases-' + uuid.uuid4().hex
    stage = (root / guest.lstrip('/')).resolve()
    if not stage.is_relative_to(root / 'var/tmp'):
        raise ValueError('fixture escaped guest var/tmp')
    stage.mkdir(parents=True)
    data, payload = package(args.files, args.children)
    (stage / 'probe.deb').write_bytes(data)
    report = dict(root=str(root), distribution=str(dist), sha256=distribution_hashes(dist),
                  files=args.files, children=args.children, results=[], passed=False)

    def save():
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    try:
        save()
        with InitPool(root, dist, output / 'session', size=1, timeout=args.timeout,
                      memory_limit_bytes=4 * 1024**3) as pool:
            if args.io_probe:
                shutil.copy2(Path(__file__).resolve().parents[1] / 'tests/guest/dpkg_io_probe.py',
                             stage / 'io-probe.py')
                result = pool.run(['/usr/bin/python3', guest + '/io-probe.py', guest + '/io'],
                                  environment=['PATH=/usr/bin:/bin', 'LC_ALL=C'])
                if result['status'] != 'passed':
                    raise AssertionError('I/O probe failed: ' + str(result))
                report['io_probe_ms'] = json.loads((stage / 'io-results.json').read_text())
                print(json.dumps(report['io_probe_ms']), flush=True)
                save()
            for iteration in range(args.repeat):
                location = stage / str(iteration)
                for directory in ('admin/updates', 'admin/info', 'install'):
                    (location / directory).mkdir(parents=True)
                (location / 'admin/status').write_bytes(b'')
                prefix = f'{guest}/{iteration}'
                base = ['/usr/bin/dpkg', '--admindir=' + prefix + '/admin',
                        '--instdir=' + prefix + '/install', '--force-script-chrootless',
                        '--log=' + prefix + '/dpkg.log']
                for phase, command in [('unpack', ['--unpack', guest + '/probe.deb']),
                                       ('configure', ['--configure', '--pending'])]:
                    print(f'{iteration}: starting {phase}', flush=True)
                    row = pool.run(base + command, environment=[
                        'PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL=C',
                        'DEBIAN_FRONTEND=noninteractive'])
                    row.update(iteration=iteration, phase=phase)
                    report['results'].append(row)
                    save()
                    print(json.dumps(dict(phase=phase, status=row['status'],
                                          ms=row['request_to_exit_ms'])), flush=True)
                    if row['status'] != 'passed':
                        raise AssertionError(f'{phase}: {row["status"]}')
                for name, expected, _ in payload:
                    if expected is not None:
                        assert (location / 'install' / name).read_bytes() == expected, name
                assert (location / 'install/configured').read_bytes() == b'configured'
                assert 'Status: install ok installed' in (location / 'admin/status').read_text()
                if args.apt:
                    apt_stage = location / 'apt'
                    for directory in ('admin/updates', 'admin/info', 'install', 'etc/parts', 'etc/sources.list.d', 'etc/preferences.d',
                                      'state/lists/partial', 'cache/archives/partial', 'log'):
                        (apt_stage / directory).mkdir(parents=True)
                    (apt_stage / 'admin/status').write_bytes(b'')
                    for name in ('apt.conf', 'sources.list'):
                        (apt_stage / 'etc' / name).write_bytes(b'')
                    apt_prefix = prefix + '/apt'
                    config = {
                        'Dir::Etc': apt_prefix + '/etc', 'Dir::Etc::main': 'apt.conf',
                        'Dir::Etc::parts': 'parts', 'Dir::Etc::sourcelist': 'sources.list',
                        'Dir::Etc::sourceparts': 'sources.list.d', 'Dir::State': apt_prefix + '/state',
                        'Dir::State::status': apt_prefix + '/admin/status',
                        'Dir::Cache': apt_prefix + '/cache', 'Dir::Log': apt_prefix + '/log',
                        'Dir::Bin::dpkg': '/usr/bin/dpkg', 'DPkg::Use-Pty': 'false',
                        'APT::Install-Recommends': 'false',
                    }
                    text = ''.join(f'{key} "{value}";\n' for key, value in config.items())
                    options = ['--admindir=' + apt_prefix + '/admin',
                               '--instdir=' + apt_prefix + '/install',
                               '--log=' + apt_prefix + '/dpkg.log', '--force-script-chrootless']
                    text += 'DPkg::Options { ' + ' '.join(f'"{value}";' for value in options) + ' };\n'
                    (apt_stage / 'config').write_text(text, encoding='utf-8', newline='\n')
                    print(f'{iteration}: starting apt-install', flush=True)
                    row = pool.run(['/usr/bin/apt-get', '-y', 'install', guest + '/probe.deb'],
                                   environment=['PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL=C',
                                                'DEBIAN_FRONTEND=noninteractive', 'APT_CONFIG=' + apt_prefix + '/config'])
                    row.update(iteration=iteration, phase='apt-install')
                    report['results'].append(row)
                    save()
                    print(json.dumps(dict(phase='apt-install', status=row['status'],
                                          ms=row['request_to_exit_ms'])), flush=True)
                    if row['status'] != 'passed':
                        raise AssertionError('apt-install: ' + row['status'])
                    for name, expected, _ in payload:
                        if expected is not None:
                            assert (apt_stage / 'install' / name).read_bytes() == expected, name
                    assert (apt_stage / 'install/configured').read_bytes() == b'configured'
                    assert 'Status: install ok installed' in (apt_stage / 'admin/status').read_text()
            report['cpu_metrics'] = pool.child.cpu_metrics()
        report['median_ms'] = {phase: statistics.median(row['request_to_exit_ms']
            for row in report['results'] if row['phase'] == phase)
            for phase in ('unpack', 'configure', *(['apt-install'] if args.apt else []))}
        report['passed'] = True
    except Exception as error:
        report['error'] = repr(error)
    finally:
        save()
        # The session Job has closed before deleting only our unique fixture.
        if not args.keep and report['passed']:
            shutil.rmtree(stage)
        else:
            report['retained_fixture'] = str(stage)
            save()
    print(json.dumps(report.get('median_ms', report.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
