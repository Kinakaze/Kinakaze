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
                         ('bun', '/usr/local/bin/bun'),
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
    add('apt-check', 'apt-get check; dpkg --audit > audit; test ! -s audit; '
        'dpkg-query -W > packages; test -s packages')
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
    add('raw-epoll-create', 'python3 RawEpollCreateProbe.py')
    add('sysv-proc', 'python3 SysvProcProbe.py', timeout=60)
    add('cond-reacquire-notify', 'gcc -O2 CondReacquireNotifyProbe.c -pthread -o cond-reacquire; '
        './cond-reacquire | grep -q COND_REACQUIRE_NOTIFY_OK', ['/usr/bin/gcc'], 15)
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
    add('tmpfs-eof', 'gcc -O0 TmpfsEofProbe.c -o tmpfs-eof-probe && ./tmpfs-eof-probe', ['/usr/bin/gcc'], 60)
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
    add('bun-filesystem', shlex.quote(args.bun) + ' BunAgentFilesystemProbe.js', [args.bun], 45)
    add('bun-subprocess', shlex.quote(args.bun) + ' BunAgentSubprocessProbe.js', [args.bun], 90)
    add('cpp-futures', 'clang++ -O2 -std=c++17 CppFutureProbe.cpp -pthread -o cpp-futures; '
        './cpp-futures | grep -q CPP_FUTURES_OK', ['/usr/bin/clang++'], 120)
    add('go', shlex.quote(args.go) + ' run program.go', [args.go], 180)
    add('cargo', '''mkdir -p src
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cat > Cargo.toml <<'EOF'
[package]
name = "compatibility-fixture"
version = "0.1.0"
edition = "2021"
EOF
cat > src/lib.rs <<'EOF'
pub fn total(values: &[i64]) -> i64 { values.iter().sum() }
#[cfg(test)] mod tests {
    use super::total;
    #[test] fn empty() { assert_eq!(total(&[]), 0); }
    #[test] fn signed() { assert_eq!(total(&[-2, 3, 7]), 8); }
    #[test] fn threads_and_processes() {
        let threads: Vec<_> = (0..8).map(|value| std::thread::spawn(move || value * value)).collect();
        assert_eq!(threads.into_iter().map(|thread| thread.join().unwrap()).sum::<i32>(), 140);
        let result = std::process::Command::new("/bin/echo").arg("child").output().unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"child\\n");
    }
}
EOF
export CARGO_HOME="$PWD/cargo-home"
cargo test --offline -- --test-threads=2 > out 2> err
cat out err
grep -q '3 passed; 0 failed' out
cargo test --offline -- --test-threads=2 > out 2> err
grep -q '3 passed; 0 failed' out
! grep -q 'Compiling compatibility-fixture' err''', ['/usr/bin/cargo', '/usr/bin/rustc', '/usr/bin/gcc'], 180)
    add('git-workflow', '''git init -q -b main repository
git -C repository config user.name Compatibility
git -C repository config user.email compatibility@example.invalid
printf 'first\\n' > repository/payload
git -C repository add payload
git -C repository commit -qm first
git -C repository checkout -qb feature
printf 'second\\n' >> repository/payload
git -C repository commit -qam second
git -C repository checkout -q main
git -C repository merge --ff-only feature
git -C repository gc --prune=now
git -C repository fsck --strict
git clone --no-hardlinks repository clone
cmp repository/payload clone/payload
test "$(git -C clone rev-list --count HEAD)" = 2
test -z "$(git -C clone status --porcelain)"
mkdir extracted
git -C clone archive HEAD | tar -x -C extracted
cmp clone/payload extracted/payload''', ['/usr/bin/git', '/bin/tar'], 120)
    add('archive-roundtrip', '''mkdir source extracted
python3 -c 'from pathlib import Path; p=Path("source"); (p/"payload").write_bytes(bytes(range(256))*4096); (p/"empty").touch(); (p/"space name").write_text("archive fixture\\n")'
tar -cf payload.tar source
gzip -c payload.tar > payload.tar.gz
gzip -dc payload.tar.gz | cmp - payload.tar
xz -c payload.tar > payload.tar.xz
xz -dc payload.tar.xz | cmp - payload.tar
zip -qr payload.zip source
unzip -q payload.zip -d extracted
diff -r source extracted/source
tar -xf payload.tar -C extracted
diff -r source extracted/source''', ['/usr/bin/python3', '/bin/tar', '/bin/gzip', '/usr/bin/xz', '/usr/bin/zip', '/usr/bin/unzip'], 90)
    add('cmake-ninja', '''cat > CMakeLists.txt <<'EOF'
cmake_minimum_required(VERSION 3.16)
project(compatibility_fixture C)
enable_testing()
add_executable(fixture main.c)
add_test(NAME output COMMAND fixture)
set_tests_properties(output PROPERTIES PASS_REGULAR_EXPRESSION "CMAKE_RUNTIME_OK")
EOF
printf '#include <stdio.h>\\nint main(void) { puts("CMAKE_RUNTIME_OK"); return 0; }\\n' > main.c
cmake -S . -B build -G Ninja
cmake --build build --parallel 4
ctest --test-dir build --output-on-failure
ninja -C build -n > out
grep -q 'no work to do' out''', ['/usr/bin/cmake', '/usr/bin/ninja', '/usr/bin/gcc'], 120)
    add('python-venv', '''python3 -m venv environment
environment/bin/python -m pip --version
environment/bin/python -c 'import sys, pathlib, subprocess; assert sys.prefix != sys.base_prefix; assert pathlib.Path(sys.prefix).name == "environment"; assert subprocess.check_output([sys.executable,"-c","print(42)"]).strip() == b"42"'
environment/bin/python -m compileall -q environment/lib''', ['/usr/bin/python3'], 120)
    add('npm-lifecycle', '''cat > package.json <<'EOF'
{"name":"compatibility-fixture","version":"1.0.0","scripts":{"pretest":"node prepare.cjs","test":"node test.cjs"}}
EOF
printf '%s' 'require("fs").writeFileSync("pretest","ok");' > prepare.cjs
cat > test.cjs <<'EOF'
const assert = require('node:assert/strict');
const fs = require('node:fs');
const child = require('node:child_process');
assert.equal(fs.readFileSync('pretest', 'utf8'), 'ok');
assert.equal(process.env.npm_lifecycle_event, 'test');
assert.equal(child.execFileSync(process.execPath, ['-p', '6*7'], {encoding:'utf8'}).trim(), '42');
console.log('NPM_LIFECYCLE_OK');
EOF
export npm_config_cache="$PWD/npm-cache"
npm --offline test > out
cat out
grep -q NPM_LIFECYCLE_OK out''', ['/usr/bin/node', '/usr/bin/npm'], 120)
    add('ruby', '''ruby -rjson -rdigest -ropen3 -e '
payload=JSON.generate({"values"=>[2,3,7]}); File.write("payload.json",payload)
raise unless JSON.parse(File.read("payload.json"))["values"].sum==12
raise unless Digest::SHA256.hexdigest(payload).size==64
raise unless (0...8).map{|value| Thread.new { value*value }}.map(&:value).sum==140
output,status=Open3.capture2("/bin/echo","ruby-child")
raise unless status.success? && output.strip=="ruby-child"
puts "RUBY_RUNTIME_OK"' ''', ['/usr/bin/ruby'], 60)
    add('php', '''php -r '$p=new PDO("sqlite::memory:"); if($p->query("select 42")->fetchColumn()!=42) exit(1);
if(json_decode(json_encode(["x"=>42]),true)["x"]!=42) exit(2); echo "PHP_OK\\n";' ''', ['/usr/bin/php'])
    add('nginx', 'python3 NginxRuntimeProbe.py', ['/usr/sbin/nginx'], 150)
    add('service-namespace-primitives', 'python3 NamespaceServicesProbe.py', ['/usr/bin/python3'], 45)
    add('redis', 'python3 RedisRuntimeProbe.py', ['/usr/bin/redis-server'], 150)
    add('postgresql', 'python3 PostgresqlRuntimeProbe.py', ['/usr/lib/postgresql/15/bin/postgres'], 180)
    add('mariadb', 'python3 MariadbRuntimeProbe.py', ['/usr/sbin/mariadbd', '/usr/bin/mariadb-install-db'], 240)
    add('sqlite-processes', 'python3 SqliteProcessBoundaryProbe.py', ['/usr/bin/python3'], 90)
    add('ffmpeg', 'python3 FfmpegRuntimeProbe.py', ['/usr/bin/ffmpeg', '/usr/bin/ffprobe'], 180)
    add('java', '/usr/lib/jvm/java-17-openjdk-amd64/bin/javac JavaRuntimeProbe.java; '
        '/usr/lib/jvm/java-17-openjdk-amd64/bin/java -cp . JavaRuntimeProbe spawn',
        ['/usr/lib/jvm/java-17-openjdk-amd64/bin/javac'], 180)
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
                                'BunAgentFilesystemProbe.js', 'BunAgentSubprocessProbe.js', 'SysvProcProbe.py', 'CondReacquireNotifyProbe.c', 'CppFutureProbe.cpp', 'RawEpollCreateProbe.py', 'MariadbRuntimeProbe.py', 'SqliteProcessBoundaryProbe.py', 'FfmpegRuntimeProbe.py', 'JavaRuntimeProbe.java',
                                'NamedSemaphoreProbe.c', 'MultiprocessingProbe.py', 'ForkDlopenProbe.c', 'IgnoredSignalIoProbe.c', 'PpollSignalProbe.c', 'AccountForkProbe.c', 'AccountStreamProbe.c', 'PathAccessProbe.py', 'PathReferenceProbe.py', 'RootResolutionProbe.py', 'GlobCallbackProbe.c', 'TmpfilesProbe.py', 'XattrLifetimeProbe.py',
                                'NginxRuntimeProbe.py', 'RedisRuntimeProbe.py', 'PostgresqlRuntimeProbe.py', 'DescriptorDuplicationProbe.py', 'DatagramRightsProbe.py', 'UnixListenerCustodyProbe.py', 'UnixSocketOptionsProbe.py', 'TmpfsMappingProbe.py', 'TmpfsEofProbe.c', 'NamespaceServicesProbe.py'):
                    shutil.copyfile(source / fixture, stage / fixture)
                (stage / 'tls.c').write_text('_Thread_local int value=40; int ready; '
                    '__attribute__((constructor)) static void init(void){ready=9;} '
                    'int increment(void){return ++value;}\n', encoding='utf-8')
                shutil.copyfile(source / 'GoRuntimeProbe.go', stage / 'program.go')
                marker = 'WORKLOAD_OK_' + name
                script = 'set -euo pipefail\nexport PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME=/root\n' + case['script'] + '\nprintf "%s\\n" ' + shlex.quote(marker)
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
