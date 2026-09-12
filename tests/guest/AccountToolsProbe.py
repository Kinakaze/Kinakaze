"""Run Debian account tools against the guest databases using disposable names."""
import grp
import os
from pathlib import Path
import pwd
import subprocess
import uuid

suffix = uuid.uuid4().hex[:12]
user, group = 'abiuser' + suffix, 'abigroup' + suffix
before = {name: Path('/etc/' + name).read_bytes() for name in ['passwd', 'group']}

def run(program, *args, expected=0):
    result = subprocess.run(['/usr/sbin/' + program, *args], stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=20)
    assert result.returncode == expected, (program, result.returncode, result.stderr.decode(errors='replace'))
    return result

try:
    run('groupadd', '--system', group)
    record = grp.getgrnam(group)
    assert record.gr_name == group and record.gr_gid != 0
    run('groupadd', '--system', group, expected=9)
    run('useradd', '--system', '--gid', group, '--no-create-home',
        '--home-dir', '/nonexistent', '--shell', '/bin/sh', user)
    account = pwd.getpwnam(user)
    assert account.pw_name == user and account.pw_uid != 0
    assert account.pw_gid == record.gr_gid and account.pw_dir == '/nonexistent'
    run('useradd', '--system', user, expected=9)
    # A primary group cannot be removed while its user still exists.
    run('groupdel', group, expected=8)
    run('userdel', user)
    run('groupdel', group)
    for lookup, name in [(pwd.getpwnam, user), (grp.getgrnam, group)]:
        try:
            lookup(name)
        except KeyError:
            pass
        else:
            raise AssertionError('deleted account remained visible: ' + name)
    for name, original in before.items():
        assert Path('/etc/' + name).read_bytes() == original, name
finally:
    # Only clean up names created by this probe if an assertion stopped it early.
    if any(line.startswith(user + ':') for line in Path('/etc/passwd').read_text().splitlines()):
        run('userdel', user)
    if any(line.startswith(group + ':') for line in Path('/etc/group').read_text().splitlines()):
        run('groupdel', group)
print('ACCOUNT_TOOLS_CREATE_DUPLICATE_PRIMARY_GROUP_DELETE_OK')
