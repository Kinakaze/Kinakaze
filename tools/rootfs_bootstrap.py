"""Configure Kinakaze's adapted base image and its real dpkg file ownership.

The selected Debian payloads and native ABI providers form one Kinakaze package.
They are not reported as separately configured Debian installations: upstream
maintainer scripts have not run. Additional packages are managed normally by APT.
"""
import hashlib
import json
from string import Template


def configure_pam(files):
    # libpam-runtime's postinst normally renders these templates. Retain the
    # shipped Unix policy (including its password hash algorithm), with no
    # dependency on a particular PID 1 or an interactive debconf session.
    profile, field = {}, None
    for line in files['usr/share/pam-configs/unix'].decode().splitlines():
        if line.startswith((' ', '\t')) and field:
            profile.setdefault(field, []).append(line.strip())
        elif ':' in line:
            field = line.split(':', 1)[0]
    for kind in ('auth', 'account', 'password', 'session', 'session-noninteractive'):
        group = kind.split('-')[0]
        rules = profile[group.title() + '-Initial']
        unix = '\n'.join(group + '\t' + rule.replace('success=end', 'success=1') for rule in rules)
        primary, additional = (group + '\t[default=1]\tpam_permit.so', unix) if group == 'session' else (unix, '')
        template = Template(files['usr/share/pam/common-' + kind].decode())
        variable = 'session_nonint' if kind.endswith('-noninteractive') else group
        files.setdefault('etc/pam.d/common-' + kind, template.substitute({
            variable + '_primary': primary, variable + '_additional': additional,
        }).encode())


def configure_service_dispatch(files):
    # --type is filtered by the systemctl client after querying unit states.
    # Supplying the equivalent name pattern also filters in the manager before
    # it scans enablement links for every unrelated service/target/timer.
    path = 'usr/sbin/service'
    if path in files:
        command = b'systemctl list-unit-files --full --type=socket'
        files[path] = files[path].replace(command + b' 2>', command + b" '*.socket' 2>")
        # Both the version and usage strings only need the script's basename.
        # Shell expansion avoids two child processes on every service action.
        files[path] = files[path].replace(b'`basename $0`', b'${0##*/}')


def configure_standard(files, links, preset):
    configure_pam(files)
    configure_service_dispatch(files)
    # Retain Debian's account IDs/shells, plus service accounts from the manifest.
    for name in ('passwd', 'group'):
        path = 'etc/' + name
        base = files['usr/share/base-passwd/' + name + '.master']
        names = {line.split(b':', 1)[0] for line in base.splitlines()}
        extra = b''.join(line + b'\n' for line in files.get(path, b'').splitlines()
                         if line.split(b':', 1)[0] not in names)
        files[path] = base + extra
    # Base-passwd ships '*' in passwd. Move authentication to the actual shadow
    # database, preserving the manifest's chosen password and locking others.
    configured = {line.split(b':', 1)[0]: line for line in files.get('etc/shadow', b'').splitlines()}
    accounts = [line.split(b':') for line in files['etc/passwd'].splitlines()]
    for fields in accounts:
        if len(fields) != 7:
            raise ValueError('invalid base passwd entry')
        fields[1] = b'x'
    files['etc/passwd'] = b''.join(b':'.join(fields) + b'\n' for fields in accounts)
    files['etc/shadow'] = b''.join(configured.get(fields[0], fields[0] + b':!:20000:0:99999:7:::') + b'\n'
                                for fields in accounts)
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
