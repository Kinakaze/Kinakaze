"""Real unprivileged PostgreSQL transactions, concurrent clients and restart."""
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import pwd
import subprocess
import tempfile

bindir = Path('/usr/lib/postgresql/15/bin')
account = next(user for user in pwd.getpwall() if user.pw_name in ('postgres', 'nobody'))


def unprivileged():
    os.setgroups([])
    os.setgid(account.pw_gid)
    os.setuid(account.pw_uid)


with tempfile.TemporaryDirectory(prefix='postgres-boundary-') as directory:
    prefix = Path(directory)
    prefix.chmod(0o755)
    data, socket = prefix / 'data', prefix / 'socket'
    for path in (data, socket):
        path.mkdir()
        os.chown(path, account.pw_uid, account.pw_gid)
    environment = dict(os.environ, PGHOST=str(socket), PGPORT='55439', PGUSER=account.pw_name,
                       PGDATABASE='postgres', LC_ALL='C', LANG='C')
    running = False

    def run(program, *arguments, timeout=60):
        result = subprocess.run([str(bindir / program), *map(str, arguments)],
            preexec_fn=unprivileged, env=environment, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout)
        assert result.returncode == 0, (program, arguments, result.returncode, result.stdout.decode(errors='replace'))
        return result.stdout.decode()

    def sql(query):
        return run('psql', '-X', '-At', '-v', 'ON_ERROR_STOP=1', '-c', query).strip()

    def start():
        global running
        run('pg_ctl', '-D', data, '-l', data / 'server.log', '-o',
            f"-h '' -k {socket} -p 55439 -c max_connections=20", '-w', 'start')
        running = True

    def stop():
        global running
        run('pg_ctl', '-D', data, '-m', 'fast', '-w', 'stop')
        running = False

    try:
        run('initdb', '-D', data, '-A', 'trust', '--no-locale')
        start()
        sql('create table probe(value int); begin; insert into probe values(42); commit;')
        sql('begin; insert into probe values(99); rollback;')
        value = sql('select sum(value) from probe')
        assert value == '42', repr(value)
        with ThreadPoolExecutor(4) as pool:
            assert list(pool.map(lambda _: sql('select sum(value) from probe'), range(8))) == ['42'] * 8
        stop()
        start()
        value = sql('select sum(value) from probe')
        assert value == '42', repr(value)
        stop()
        print('POSTGRES_TRANSACTIONS_RESTART_OK', flush=True)
    finally:
        try:
            if running:
                # Preserve the original failure; cleanup must not repeat a
                # potentially blocked fast shutdown or hide its diagnostics.
                try:
                    run('pg_ctl', '-D', data, '-m', 'immediate', '-t', '10', '-w', 'stop', timeout=15)
                except Exception as error:
                    print(f'POSTGRES_CLEANUP_FAILED: {error}', flush=True)
        finally:
            if (data / 'server.log').is_file():
                print((data / 'server.log').read_text(errors='replace')[-12000:], flush=True)
