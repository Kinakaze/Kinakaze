"""Build standard desktop caches with the guest tools after preparing and packaging a root."""
import argparse
from pathlib import Path
import subprocess
import uuid


def prepare_service_accounts(root):
    """Create package service identities inside this guest, without maintainer scripts."""
    passwd, group = root / 'etc/passwd', root / 'etc/group'
    users = passwd.read_text(encoding='utf-8').splitlines()
    groups = group.read_text(encoding='utf-8').splitlines()
    user_names = {row.split(':')[0] for row in users}
    group_ids = {row.split(':')[0]: int(row.split(':')[2]) for row in groups if len(row.split(':')) == 4}
    occupied = {int(row.split(':')[2]) for row in users if len(row.split(':')) == 7} | set(group_ids.values())
    for name, binary, home in (
        ('polkitd', 'usr/lib/polkit-1/polkitd', '/var/lib/polkit-1'),
        ('colord', 'usr/libexec/colord', '/var/lib/colord'),
        ('geoclue', 'usr/libexec/geoclue', '/var/lib/geoclue'),
    ):
        if not (root / binary).is_file():
            continue
        if name not in user_names:
            uid = next(value for value in range(200, 999) if value not in occupied)
            occupied.add(uid)
            gid = group_ids.get(name, uid)
            if name not in group_ids:
                groups.append(f'{name}:x:{gid}:')
                group_ids[name] = gid
            users.append(f'{name}:x:{uid}:{gid}:{name}:{home}:/usr/sbin/nologin')
            user_names.add(name)
        (root / home.lstrip('/')).mkdir(parents=True, exist_ok=True)
    if (root / 'usr/libexec/accounts-daemon').is_file():
        for subdirectory in ('users', 'icons'):
            (root / 'var/lib/AccountsService' / subdirectory).mkdir(parents=True, exist_ok=True)
    passwd.write_text('\n'.join(users) + '\n', encoding='utf-8', newline='\n')
    group.write_text('\n'.join(groups) + '\n', encoding='utf-8', newline='\n')


def prepare_session_defaults(root, timezone=None):
    if timezone:
        zones = (root / 'usr/share/zoneinfo').resolve()
        source = (zones / timezone).resolve()
        if not source.is_relative_to(zones) or not source.is_file():
            raise ValueError('Unknown installed timezone: ' + timezone)
        (root / 'etc/localtime').write_bytes(source.read_bytes())
        (root / 'etc/timezone').write_text(timezone + '\n', encoding='utf-8', newline='\n')
    if (root / 'bin/bash').is_file():
        passwd = root / 'etc/passwd'
        rows = []
        for row in passwd.read_text(encoding='utf-8').splitlines():
            fields = row.split(':')
            if len(fields) == 7 and fields[0] == 'root' and fields[6] == '/bin/sh':
                fields[6] = '/bin/bash'
                row = ':'.join(fields)
            rows.append(row)
        passwd.write_text('\n'.join(rows) + '\n', encoding='utf-8', newline='\n')
        for name in ('.bashrc', '.profile'):
            source, destination = root / 'etc/skel' / name, root / 'root' / name
            if source.is_file() and not destination.exists():
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(source.read_bytes())
    # GTK's solid-CSD fallback adds a 5px frame when no X compositor owns its
    # display. Native windows already supply resize hit testing and rounding.
    styles = {
        'gtk-3.0': '''decoration {
  padding: 0; margin: 0; border: none; box-shadow: none;
}
window { border: none; box-shadow: none; outline: none; }
window.solid-csd headerbar.titlebar { margin: 0; }
''',
        'gtk-4.0': '''window { border: none; box-shadow: none; outline: none; padding: 0; margin: 0; }
''',
    }
    for toolkit, css in styles.items():
        directory = root / 'root/.config' / toolkit
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'kinakaze-window.css').write_text(css, encoding='utf-8', newline='\n')
        main = directory / 'gtk.css'
        existing = main.read_text(encoding='utf-8') if main.exists() else ''
        include = '@import url("kinakaze-window.css");'
        if include not in existing:
            main.write_text(include + '\n' + existing, encoding='utf-8', newline='\n')


def prepare(dist, root=None, timezone=None):
    dist = dist.resolve()
    root = (root if root is not None else dist / 'rootfs').resolve()
    worker = dist / 'worker.exe'
    if not worker.is_file() or not root.is_dir():
        raise FileNotFoundError('Packaged worker.exe and rootfs are required')
    prepare_service_accounts(root)
    prepare_session_defaults(root, timezone)

    def run(program, *args):
        if not (root / program.lstrip('/')).is_file():
            raise FileNotFoundError(program)
        print('Preparing:', program, *args, flush=True)
        subprocess.run([str(worker), 'run', '--root', str(root), '--dist', str(dist),
                        '--', program, *args], check=True, timeout=90,
                       creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))

    schemas = root / 'usr/share/glib-2.0/schemas'
    if schemas.is_dir() and any(schemas.glob('*.gschema.xml')):
        run('/usr/bin/glib-compile-schemas', '--strict', '/usr/share/glib-2.0/schemas')
    for library in sorted((root / 'usr/lib').glob('*/gdk-pixbuf-2.0')):
        run('/' + (library / 'gdk-pixbuf-query-loaders').relative_to(root).as_posix(), '--update-cache')
    for modules in sorted((root / 'usr/lib').glob('*/gio/modules')):
        if any(modules.glob('*.so')):
            run('/usr/bin/gio-querymodules', '/' + modules.relative_to(root).as_posix())
    if (root / 'usr/share/mime/packages').is_dir():
        run('/usr/bin/update-mime-database', '/usr/share/mime')
    if (root / 'etc/fonts/fonts.conf').is_file():
        run('/usr/bin/fc-cache', '--force')
    if (root / 'usr/bin/ibus').is_file() and (root / 'usr/share/ibus/component').is_dir():
        # Packages added after the first daemon start must become visible to
        # its default registry lookup, including the dconf config component.
        run('/usr/bin/ibus', 'write-cache', '--system')
    if (root / 'usr/bin/dconf').is_file() and (root / 'etc/dconf/db').is_dir():
        run('/usr/bin/dconf', 'update')

    # XDG base directories belong to login homes; service accounts have no desktop.
    for line in (root / 'etc/passwd').read_text(encoding='utf-8').splitlines():
        fields = line.split(':')
        if len(fields) != 7 or fields[6].endswith(('/nologin', '/false')):
            continue
        home = fields[5]
        if not home.startswith('/'):
            raise ValueError('Non-absolute login home: ' + home)
        for name in ('.config', '.cache', '.local/share'):
            target = (root / home.lstrip('/') / name).resolve()
            if not target.is_relative_to(root):
                raise ValueError('Login home escapes root: ' + home)
            target.mkdir(parents=True, exist_ok=True)
    machine_id = root / 'etc/machine-id'
    try:
        with machine_id.open('x', encoding='ascii', newline='\n') as stream:
            stream.write(uuid.uuid4().hex + '\n')
    except FileExistsError:
        value = machine_id.read_text(encoding='ascii').strip()
        if len(value) != 32 or any(c not in '0123456789abcdef' for c in value):
            raise ValueError('Existing /etc/machine-id is not a valid machine ID')
        # Windows text writes must not leave a CR in this Linux data format.
        machine_id.write_bytes((value + '\n').encode('ascii'))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--root', type=Path, help='Separate guest root; defaults to DIST/rootfs')
    parser.add_argument('--timezone', help='Installed IANA timezone, for example Asia/Shanghai')
    args = parser.parse_args()
    prepare(args.dist, args.root, args.timezone)
