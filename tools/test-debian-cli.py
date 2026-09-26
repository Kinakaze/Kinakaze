"""Exercise the standard preset's daily commands in a freshly installed guest."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess

temporary_root = runpy.run_path(str(Path(__file__).with_name('test-first-run.py')))['temporary_root']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    dist = args.dist.resolve()
    checks = []
    with temporary_root() as directory:
        root = Path(directory) / 'standard root'
        env = {k: v for k, v in os.environ.items() if not k.startswith('KINAKAZE_')}
        env['PATH'] = ''

        def invoke(arguments, input=None, timeout=120):
            result = subprocess.run([str(dist / 'worker.exe'), '--root', str(root), *arguments],
                                    input=input, capture_output=True, timeout=timeout, env=env,
                                    creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
            if result.returncode:
                raise AssertionError(f'{arguments}: {result.returncode}\n{result.stdout.decode(errors="replace")}\n{result.stderr.decode(errors="replace")}')
            return result.stdout.decode('utf-8', errors='replace')

        assert invoke([], b'printf "LOGIN=%s:%s:%s\\n" "$BASH_VERSION" "$HOME" "$PWD"\nexit\n').startswith('LOGIN=5.')
        checks.append('offline first login selects Bash and /root from passwd')
        scripts = {
            'Bash arrays, pipefail, process substitution and completion': '''
test -n "$BASH_VERSION"; test "$HOME:$PWD:$SHELL" = /root:/root:/bin/bash
array=(one two); test "${array[1]}" = two
set -o pipefail; if false | true; then exit 1; fi
diff <(printf same) <(printf same)
. /usr/share/bash-completion/bash_completion; declare -F _init_completion
''',
            'case-sensitive names, UTF-8 locale, UTC default and basic commands': '''
mkdir -p /tmp/names; printf upper > /tmp/names/Name; printf lower > /tmp/names/name
test "$(cat /tmp/names/Name)" = upper; test "$(cat /tmp/names/name)" = lower
test "$(locale charmap)" = UTF-8; test "$(date -d @0 +%H:%M)" = 00:00
printf 'c\\na\\nb\\n' | sort | uniq | wc -l | grep -q 3
find /tmp/names -type f | wc -l | grep -q 2
''',
            'Debian manuals render through man and groff': 'man -P cat ls > /tmp/ls-man; grep -q SYNOPSIS /tmp/ls-man',
            'editor and pager alternatives remain configurable': '''
nano --version; vim.tiny --version; less --version
test "$(readlink /etc/alternatives/editor)" = /bin/nano
update-alternatives --set editor /usr/bin/vim.tiny
test "$(readlink /etc/alternatives/editor)" = /usr/bin/vim.tiny
update-alternatives --auto editor
test "$(readlink /etc/alternatives/editor)" = /bin/nano
''',
            'Perl standard modules and temporary file IO': '''
perl -MFile::Temp=tempfile -MJSON::PP -MPOSIX -e 'my ($h,$p)=tempfile(UNLINK=>1); print $h "works"; close $h; open my $r,"<",$p or die $!; die unless <$r> eq "works"; print encode_json({ok=>1}),"\\n";'
''',
            'Python SSL, SQLite, JSON and subprocess': '''
python3 -c 'import ssl,sqlite3,json,subprocess; db=sqlite3.connect(":memory:"); assert db.execute("select 42").fetchone()==(42,); assert subprocess.check_output(["printf","ok"])==b"ok"; print(json.dumps({"ssl":ssl.OPENSSL_VERSION}))'
''',
            'NSS account, group-shadow, aliases, hosts and ethers queries': '''
getent passwd root | grep -q '^root:.*:0:0:'
getent group root | grep -q '^root:.*:0:'
getent gshadow root | grep -q '^root:!::'
test "$(stat -c %a /etc/gshadow)" = 600
getent -s files hosts 127.0.0.1 | grep -q localhost
getent -s hosts:files hosts localhost | grep -q localhost
printf 'team: alice, bob,\\n  carol\\n' > /etc/aliases
getent aliases team | grep -q carol
printf '02:00:00:00:00:ff printer\\n' > /etc/ethers
getent ethers printer | grep -q printer
getent ethers 02:00:00:00:00:ff | grep -q printer
getent protocols tcp; getent services http; getent rpc portmapper
''',
            'network queries and cross-process cwd, root, executable and file links': '''
curl --version; wget --version; ssh -V
ip -j addr show > /tmp/ip-addresses.json 2> /tmp/ip-errors
test ! -s /tmp/ip-errors
python3 -c 'import json; links=json.load(open("/tmp/ip-addresses.json")); assert links and any(x.get("addr_info") for x in links)'
ps -p $$ -o args=; free -m
exec 3< /etc/passwd
lsof -p $$ -Ffn > /tmp/open-files
grep -A1 '^fcwd$' /tmp/open-files | grep -q '^n/root$'
grep -A1 '^frtd$' /tmp/open-files | grep -q '^n/$'
grep -A1 '^ftxt$' /tmp/open-files | grep -q '^n/bin/bash$'
grep -A1 '^f3$' /tmp/open-files | grep -q '^n/etc/passwd$'
exec 3<&-
''',
            'archive round trips, file identification and package accounting': '''
printf archive-data > /tmp/plain
gzip -c /tmp/plain | gzip -d > /tmp/restored; cmp /tmp/plain /tmp/restored
bzip2 -c /tmp/plain | bzip2 -d > /tmp/restored; cmp /tmp/plain /tmp/restored
xz -c /tmp/plain | xz -d > /tmp/restored; cmp /tmp/plain /tmp/restored
tar -cf /tmp/check.tar -C /tmp plain; mkdir /tmp/unpack; tar -xf /tmp/check.tar -C /tmp/unpack; cmp /tmp/plain /tmp/unpack/plain
file /bin/bash | grep -q ELF
dpkg-query -S /usr/bin/man /bin/bash; apt-get check; dpkg --audit
''',
        }
        for name, script in scripts.items():
            invoke(['--', '/bin/bash', '-lc', 'set -eu\ncd /root\n' + script])
            checks.append(name)
            print('PASS: ' + name, flush=True)
    report = dict(passed=True, checks=checks, images={
        name: hashlib.sha256((dist / name).read_bytes()).hexdigest()
        for name in ('init.exe', 'worker.exe', 'rootfs.manifest.json', 'kinakaze.cmd')})
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
