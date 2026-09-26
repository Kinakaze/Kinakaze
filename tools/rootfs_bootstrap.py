"""Configure Kinakaze's adapted base image and its real dpkg file ownership.

The selected Debian payloads and native ABI providers form one Kinakaze package.
They are not reported as separately configured Debian installations: upstream
maintainer scripts have not run. Additional packages are managed normally by APT.
"""
import hashlib
import json


def configure_packages(files, locked, packages, native_packages, version):
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
    owned = sorted(name for name in files if not name.startswith(('var/lib/dpkg/', 'var/cache/', 'var/log/')))
    conffiles = [name for name in owned if name.startswith('etc/')]
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
    files['var/lib/dpkg/info/kinakaze-base.md5sums'] = ''.join(f'{digest(name)}  {name}\n' for name in owned).encode()
    files['var/lib/dpkg/info/kinakaze-base.conffiles'] = ''.join('/' + name + '\n' for name in conffiles).encode()
