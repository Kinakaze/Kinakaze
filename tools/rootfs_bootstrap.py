"""Configure Kinakaze's adapted base image and its real dpkg file ownership.

The selected Debian payloads and native ABI providers form one Kinakaze package.
They are not reported as separately configured Debian installations: upstream
maintainer scripts have not run. Additional packages are managed normally by APT.
"""
import hashlib
import json


def configure_standard(files, links, preset):
    # Use Debian's own base account database, including the APT sandbox user.
    for name in ('passwd', 'group'):
        files['etc/' + name] = files['usr/share/base-passwd/' + name + '.master']
    # Package postinst normally creates this database. Preserve locked passwords
    # and the base-passwd member lists in the offline image.
    files['etc/gshadow'] = b''.join(fields[0] + b':!::' + fields[3] + b'\n'
        for line in files['etc/group'].splitlines() if len(fields := line.split(b':')) == 4)
    for name in ('.profile', '.bashrc', '.bash_logout'):
        files['root/' + name] = files['etc/skel/' + name]
    files['etc/shells'] = b'/bin/sh\n/bin/dash\n/bin/bash\n/bin/rbash\n'
    files['etc/timezone'] = b'Etc/UTC\n'
    links['etc/localtime'] = '/usr/share/zoneinfo/Etc/UTC'
    files['etc/profile'] = files['etc/profile'].replace(b'export SHELL=/bin/sh', b'export SHELL=/bin/bash')
    # The runtime currently reads POSIX TZ rules, not IANA TZif files.
    files['etc/profile'] += b': "${TZ:=UTC0}"\nexport TZ\n'
    files['etc/default/locale'] = b'LANG=C.UTF-8\n'
    files['etc/locale.gen'] = b'# C.UTF-8 is supplied by libc-bin; uncomment other locales as needed.\n'
    # APT-installed service packages must not start daemons as a side effect of
    # installation in a hosted process tree. Administrators can edit this policy.
    files['usr/sbin/policy-rc.d'] = b'#!/bin/sh\nexit 101\n'
    for name, alternative in preset.get('alternatives', {}).items():
        link, choices = alternative['link'], alternative['choices']
        choices = {path: priority for path, priority in choices.items() if path.lstrip('/') in files}
        if not choices:
            raise ValueError('no installed alternative for ' + name)
        selected = max(choices, key=choices.get)
        links[link.lstrip('/')] = '/etc/alternatives/' + name
        links['etc/alternatives/' + name] = selected
        # update-alternatives' documented administrative file layout, with no
        # slave links. Its real --query/--set operations are acceptance-tested.
        files['var/lib/dpkg/alternatives/' + name] = (
            'auto\n' + link + '\n\n' + ''.join(path + '\n' + str(priority) + '\n'
            for path, priority in sorted(choices.items())) + '\n').encode()


def configure_packages(files, locked, packages, native_packages, version, links=None):
    links = links if links is not None else {}
    certificates = sorted(name for name in files if name.startswith('usr/share/ca-certificates/') and name.endswith('.crt'))
    if not certificates:
        raise ValueError('base image has no CA certificates')
    bundle = b'\n'.join(files[name].rstrip() for name in certificates) + b'\n'
    for name in ('etc/ssl/certs/ca-certificates.crt', 'usr/lib/ssl/cert.pem'):
        files[name] = bundle
    files['etc/ca-certificates.conf'] = ''.join(name.removeprefix('usr/share/ca-certificates/') + '\n' for name in certificates).encode()
    records = [dict((key, locked[name][key]) for key in ('package', 'version', 'architecture', 'url', 'sha256', 'source_package', 'source_version', 'source_directory')) for name in packages]
    files['usr/share/kinakaze/bootstrap-packages.json'] = (json.dumps(dict(schema=1, packages=records, native_packages=native_packages), indent=2) + '\n').encode()
    provided = sorted(set(packages) | set(native_packages))
    provides = ', '.join(f'{name} (= {locked[name]["version"]})' for name in provided)
    owned = sorted(name for name in set(files) | set(links) if not name.startswith(('var/lib/dpkg/', 'var/cache/', 'var/log/')))
    conffiles = [name for name in owned if name.startswith('etc/') and name in files]
    digest = lambda name: hashlib.md5(files[name], usedforsecurity=False).hexdigest()
    control = (f'Package: kinakaze-base\nStatus: install ok installed\nPriority: required\n'
               f'Section: misc\nEssential: yes\nMaintainer: mitsukina <mitsukazee@outlook.com>\n'
               f'Architecture: amd64\nVersion: {version}\nProvides: {provides}\n'
               f'Description: Kinakaze native runtime and adapted Debian base tools\n'
               f' Bundled components are recorded in /usr/share/kinakaze/bootstrap-packages.json.\n'
               f'Conffiles:\n' + ''.join(f' /{name} {digest(name)}\n' for name in conffiles) + '\n')
    files['var/lib/dpkg/status'] = control.encode()
    files['var/lib/dpkg/available'] = b''
    files['var/lib/dpkg/arch'] = b'amd64\n'
    files['var/lib/dpkg/info/kinakaze-base.list'] = ('/.\n' + ''.join('/' + name + '\n' for name in owned)).encode()
    files['var/lib/dpkg/info/kinakaze-base.md5sums'] = ''.join(f'{digest(name)}  {name}\n' for name in owned if name in files).encode()
    files['var/lib/dpkg/info/kinakaze-base.conffiles'] = ''.join('/' + name + '\n' for name in conffiles).encode()
