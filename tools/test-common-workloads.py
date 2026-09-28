"""Run real command workloads in owned sessions, preserving timings and failures.

Missing applications are reported separately and make the full run incomplete.
Timings include init, worker, guest work and teardown; they are not pure startup.
"""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import shutil
import statistics
import uuid

from init_pool import distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('root', 'dist', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    parser.add_argument('--only', help='regular expression selecting case names')
    parser.add_argument('--repeat', type=int, default=1)
    for key, default in [('node', '/usr/bin/node'), ('go', '/usr/bin/go'),
                         ('codex', '/usr/local/bin/codex'), ('claude', '/usr/local/bin/claude')]:
        parser.add_argument('--' + key, default=default, help='absolute guest executable path')
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('repeat must be positive')
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parents[1] / 'tests/guest'
    execute = runpy.run_path(str(Path(__file__).with_name('test-debian-commands.py')))['execute']
    env = {key: value for key, value in os.environ.items() if not key.startswith(
        ('KINAKAZE_', 'OPENAI_', 'ANTHROPIC_', 'CODEX_', 'CLAUDE_'))}
    env.update(TERM='xterm-256color', LC_ALL='C.UTF-8', TZ='UTC0')
    cases = []

    def add(name, script, required=(), timeout=45, scope='functional'):
        cases.append(dict(name=name, script=script, required=list(required), timeout=timeout, scope=scope))

    add('shell-spawn', 'for i in $(seq 1 20); do /bin/true; done; '
        'printf "c\\na\\nb\\n" | sort | uniq > sorted; test "$(cat sorted)" = "$(printf "a\\nb\\nc")"')
    add('apt-check', 'apt-get check; apt-cache show bash > package; grep -q "Package: bash" package')
    add('apt-inventory', "apt list --installed > packages; dpkg-query -W -f='${Package}\\n' > installed; "
        'test -s installed; while read package; do grep -q "^$package/" packages; done < installed')
    add('ps', 'ps -p $$ -o pid=,comm= > process; grep -q bash process; ps -eLf > threads; test -s threads')
    add('top', 'top -b -n 2 -d 0.1 > snapshots; test "$(grep -c Tasks: snapshots)" = 2')
    for editor in ('vi', 'vim'):
        add(editor, f"printf 'hello\\n' > text; {editor} -e -s -u NONE -i NONE text "
            "+'s/hello/edited/' +wq; grep -qx edited text", ['/usr/bin/' + editor])
    add('python', '''python3 -c 'import json,ssl,sqlite3,subprocess,concurrent.futures
db=sqlite3.connect(":memory:"); assert db.execute("select 42").fetchone()==(42,)
assert subprocess.check_output(["/bin/echo","child"])==b"child\\n"
with concurrent.futures.ThreadPoolExecutor(4) as p: assert sum(p.map(lambda n:n*n,range(100)))==328350
print(json.dumps({"ssl":ssl.OPENSSL_VERSION}))' ''')
    add('statfs-boundaries', 'python3 StatfsBoundaryProbe.py')
    add('standard-handle-lifetimes', 'python3 StandardHandleLifetimeProbe.py')
    add('native-permissions', 'python3 NativePermissionProbe.py')
    add('directory-types', 'python3 DirectoryTypeProbe.py')
    add('directory-cursors', 'python3 DirectoryCursorProbe.py')
    add('metadata-paths', 'python3 MetadataPathProbe.py')
    add('path-access', 'python3 PathAccessProbe.py')
    add('path-references', 'python3 PathReferenceProbe.py')
    add('root-resolution', 'python3 RootResolutionProbe.py')
    add('xattr-lifetimes', 'python3 XattrLifetimeProbe.py')
    add('descriptor-duplication', 'python3 DescriptorDuplicationProbe.py')
    add('datagram-rights', 'python3 DatagramRightsProbe.py', timeout=90)
    add('unix-listener-custody', 'python3 UnixListenerCustodyProbe.py', timeout=60)
    add('unix-socket-options', 'python3 UnixSocketOptionsProbe.py')
    add('duplication-performance', 'python3 DescriptorDuplicationProbe.py --benchmark')
    add('xattr-performance', 'python3 XattrLifetimeProbe.py --benchmark')
    add('glob-callbacks', 'gcc -O2 GlobCallbackProbe.c -o glob-callbacks; '
        './glob-callbacks | grep -q GLOB_CALLBACKS_OK', ['/usr/bin/gcc'], 30)
    add('glob-native', 'gcc -O2 GlobCallbackProbe.c -o glob-native; '
        './glob-native native', ['/usr/bin/gcc'], 60)
    add('tmpfiles', 'python3 TmpfilesProbe.py', ['/bin/systemd-tmpfiles'])
    add('tmpfs-mapping', 'python3 TmpfsMappingProbe.py')
    add('sysusers', 'systemd-sysusers --dry-run basic.conf systemd-journal.conf '
        'systemd-network.conf', ['/bin/systemd-sysusers'])
    add('process-limits', 'python3 ProcessLimitsProbe.py')
    add('child-signals', 'python3 ChildSignalProbe.py')
    add('shared-listener', 'python3 SharedListenerProbe.py')
    add('shared-socket-waits', 'python3 SharedSocketWaitProbe.py')
    add('posix-semaphores', 'gcc -O2 PosixSemaphoreProbe.c -pthread -o semaphore; '
        './semaphore | grep -q SEMAPHORE_SHARED_FORK_EXEC_ALIAS_OK', ['/usr/bin/gcc'], 60)
    add('abort-status', 'gcc -O2 AbortStatusProbe.c -o abort-status; '
        './abort-status | grep -q ABORT_SIGNAL_STATUS_OK', ['/usr/bin/gcc'], 30)
    add('named-semaphores', 'gcc -O2 NamedSemaphoreProbe.c -pthread -o named-semaphore; '
        './named-semaphore | grep -q NAMED_SEMAPHORE_LIFETIME_OK', ['/usr/bin/gcc'], 30)
    add('python-multiprocessing', 'python3 MultiprocessingProbe.py', timeout=90)
    add('fork-dlopen', 'gcc -O2 -fPIC -shared tls.c -o fork-tls.so; '
        'gcc -O2 ForkDlopenProbe.c -ldl -o fork-dlopen; '
        './fork-dlopen ./fork-tls.so | grep -q FORK_DLOPEN_OK', ['/usr/bin/gcc'], 30)
    add('ignored-signal-io', 'gcc -O2 IgnoredSignalIoProbe.c -o ignored-signal-io; '
        './ignored-signal-io | grep -q IGNORED_SIGNAL_IO_OK', ['/usr/bin/gcc'], 30)
    add('ppoll-signals', 'gcc -O2 PpollSignalProbe.c -o ppoll-signals; '
        './ppoll-signals | grep -q PPOLL_SIGNAL_OK', ['/usr/bin/gcc'], 30)
    add('account-fork', 'gcc -O2 AccountForkProbe.c -o account-fork; '
        './account-fork | grep -q ACCOUNT_FORK_OK; '
        'sg root -c "id -g" | grep -qx 0', ['/usr/bin/gcc', '/usr/bin/sg'], 30)
    add('account-streams', 'gcc -O2 AccountStreamProbe.c -o account-streams; '
        './account-streams | grep -q ACCOUNT_STREAM_EOF_OK', ['/usr/bin/gcc'], 30)
    add('script-pty', "script -q -e -c 'printf pty-ok; stty -a; tty' transcript > output; "
        "grep -q pty-ok output; grep -q /dev/pts/ output; "
        "set +e; script -q -e -c 'exit 37' /dev/null; result=$?; test $result = 37",
        ['/usr/bin/script', '/bin/stty', '/usr/bin/tty'])
    add('login-pam', "set -x; test \"$(su -s /bin/sh nobody -c 'id -u')\" = 65534; "
        "test \"$(runuser -u nobody -- id -u)\" = 65534; passwd -S root | grep -q '^root P '; "
        "python3 PamRuntimeProbe.py",
        ['/bin/su', '/sbin/runuser'])
    for compiler in ('gcc', 'clang'):
        add(compiler, f'{compiler} -O2 -fPIC -shared tls.c -o tls.so; '
            f'{compiler} -O2 CompilerRuntimeProbe.c -ldl -lm -pthread -o program; '
            './program ./tls.so | grep -q COMPILER_RUNTIME_OK', ['/usr/bin/' + compiler], 120)
    add('dlopen-deepbind', '''gcc -DDEPENDENCY -fPIC -shared DeepBindProbe.c -o libdep.so
gcc -DPLUGIN -fPIC -shared DeepBindProbe.c -L. -ldep -Wl,-rpath,'$ORIGIN' -o plain.so
cp plain.so deep.so
gcc -rdynamic DeepBindProbe.c -ldl -o deepbind
./deepbind | grep -q DEEPBIND_SCOPE_OK''', ['/usr/bin/gcc'], 120)
    add('interpreter-command', 'python3 InterpreterCommandProbe.py', ['/usr/bin/gcc', '/usr/bin/ldd'], 90)
    add('executable-tls', '''gcc -O2 -fPIE -pie ExecutableTlsProbe.c -pthread -o tls-pie
./tls-pie | grep -q EXECUTABLE_TLS_OK
gcc -O2 -fno-pie -no-pie ExecutableTlsProbe.c -pthread -o tls-exec
./tls-exec | grep -q EXECUTABLE_TLS_OK''', ['/usr/bin/gcc'], 120)
    add('node', shlex.quote(args.node) + ' NodeRuntimeProbe.js', [args.node], 120)
    add('go', shlex.quote(args.go) + ' run program.go', [args.go], 180)
    add('php', '''php -r '$p=new PDO("sqlite::memory:"); if($p->query("select 42")->fetchColumn()!=42) exit(1);
if(json_decode(json_encode(["x"=>42]),true)["x"]!=42) exit(2); echo "PHP_OK\\n";' ''', ['/usr/bin/php'])
    add('nginx', 'python3 NginxRuntimeProbe.py', ['/usr/sbin/nginx'], 150)
    add('service-namespace-primitives', 'python3 NamespaceServicesProbe.py', ['/usr/bin/python3'], 45)
    add('redis', 'python3 RedisRuntimeProbe.py', ['/usr/bin/redis-server'], 150)
    add('postgresql', 'python3 PostgresqlRuntimeProbe.py', ['/usr/lib/postgresql/15/bin/postgres'], 180)
    for app in ('codex', 'claude'):
        path = getattr(args, app)
        add(app + '-startup', shlex.quote(path) + ' --version; ' + shlex.quote(path) + ' --help > help; test -s help',
            [path], 60, 'startup-and-help')

    selected = [case for case in cases if not args.only or re.search(args.only, case['name'])]
    if not selected:
        parser.error('no cases selected')
    report = dict(scope=__doc__, root=str(root), dist=str(dist), sha256=distribution_hashes(dist),
                  results=[], summary={}, complete=False)
    def save():
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    save()
    for iteration in range(args.repeat):
        for case in selected:
            name = case['name']
            row = dict(name=name, round=iteration, scope=case['scope'])
            missing = [path for path in case['required'] if not (root / path.lstrip('/')).is_file()]
            if missing:
                row.update(status='missing_application', missing=missing)
            else:
                guest = '/tmp/common-workloads-' + uuid.uuid4().hex
                stage = root / guest.lstrip('/')
                stage.mkdir(parents=True)
                for fixture in ('InterpreterCommandProbe.py', 'CompilerRuntimeProbe.c', 'AbortStatusProbe.c', 'PosixSemaphoreProbe.c', 'DeepBindProbe.c', 'ExecutableTlsProbe.c', 'StatfsBoundaryProbe.py', 'StandardHandleLifetimeProbe.py', 'NativePermissionProbe.py', 'DirectoryTypeProbe.py', 'DirectoryCursorProbe.py', 'MetadataPathProbe.py', 'ProcessLimitsProbe.py', 'ChildSignalProbe.py', 'SharedListenerProbe.py', 'SharedSocketWaitProbe.py', 'PamRuntimeProbe.py', 'NodeRuntimeProbe.js',
                                'NamedSemaphoreProbe.c', 'MultiprocessingProbe.py', 'ForkDlopenProbe.c', 'IgnoredSignalIoProbe.c', 'PpollSignalProbe.c', 'AccountForkProbe.c', 'AccountStreamProbe.c', 'PathAccessProbe.py', 'PathReferenceProbe.py', 'RootResolutionProbe.py', 'GlobCallbackProbe.c', 'TmpfilesProbe.py', 'XattrLifetimeProbe.py',
                                'NginxRuntimeProbe.py', 'RedisRuntimeProbe.py', 'PostgresqlRuntimeProbe.py', 'DescriptorDuplicationProbe.py', 'DatagramRightsProbe.py', 'UnixListenerCustodyProbe.py', 'UnixSocketOptionsProbe.py', 'TmpfsMappingProbe.py', 'NamespaceServicesProbe.py'):
                    shutil.copyfile(source / fixture, stage / fixture)
                (stage / 'tls.c').write_text('_Thread_local int value=40; int ready; '
                    '__attribute__((constructor)) static void init(void){ready=9;} '
                    'int increment(void){return ++value;}\n', encoding='utf-8')
                shutil.copyfile(source / 'GoRuntimeProbe.go', stage / 'program.go')
                marker = 'WORKLOAD_OK_' + name
                script = 'set -eu\nexport PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME=/root\n' + case['script'] + '\nprintf "%s\\n" ' + shlex.quote(marker)
                row.update(execute([str(dist / 'worker.exe'), 'oneshot', '--root', str(root), '--dist', str(dist),
                    '--cwd', guest, '--', '/bin/bash', '-c', script], output / f'{name}-{iteration}', env,
                    timeout=case['timeout']))
                row.update(fixture=guest, status='timeout' if row['timed_out'] else
                    'passed' if row['exit_code'] == 0 and marker in row['stdout'] else 'failed')
            report['results'].append(row)
            print(json.dumps({key: row[key] for key in ('name','round','status','seconds','exit_code') if key in row}), flush=True)
            save()
    for name in [case['name'] for case in selected]:
        rows = [row for row in report['results'] if row['name'] == name]
        values = [row['seconds'] for row in rows if row['status'] == 'passed']
        report['summary'][name] = dict(counts=dict(Counter(row['status'] for row in rows)),
            median_seconds=statistics.median(values) if values else None)
    report['complete'] = all(row['status'] == 'passed' for row in report['results'])
    save()
    return int(not report['complete'])


if __name__ == '__main__':
    raise SystemExit(main())
