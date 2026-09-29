"""Oracle MySQL initialization, transactions, concurrent clients and crash recovery."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True)
    parser.add_argument('--log-path', type=Path)
    parser.add_argument('--shutdown-timeout', type=int, default=30)
    args = parser.parse_args()
    if args.shutdown_timeout <= 0:
        parser.error('shutdown timeout must be positive')
    prefix = args.prefix.resolve()
    server = prefix / 'usr/sbin/mysqld'
    client = prefix / 'usr/bin/mysql'
    timings = {}

    def command(argv, timeout=90):
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout)
        assert result.returncode == 0, (argv, result.returncode, result.stdout.decode(errors='replace'),
                                       result.stderr.decode(errors='replace'))
        return result.stdout.decode().strip()

    version = command([str(server), '--no-defaults', '--version'])
    assert 'MySQL Community Server' in version and '8.4.11' in version, version
    print(version, flush=True)
    with tempfile.TemporaryDirectory(prefix='oracle-mysql-data-') as directory:
        root = Path(directory)
        data, address, log_path = root / 'data', root / 'mysql.sock', root / 'server.log'
        if args.log_path:
            log_path = args.log_path.resolve()
        import_directory = root / 'files'
        import_directory.mkdir()
        process = None
        options = [str(server), '--no-defaults', '--basedir=' + str(prefix / 'usr'),
                   '--datadir=' + str(data), '--user=' + str(os.getuid()),
                   '--innodb-buffer-pool-size=64M', '--secure-file-priv=' + str(import_directory)]

        def sql(query):
            return command([str(client), '--no-defaults', '--protocol=socket', '--socket=' + str(address),
                            '--user=root', '--batch', '--skip-column-names', '-e', query], timeout=30)

        def start(label):
            nonlocal process
            begin = time.monotonic()
            with log_path.open('ab') as log:
                process = subprocess.Popen([*options, '--socket=' + str(address),
                    '--pid-file=' + str(root / 'server.pid'), '--skip-networking', '--mysqlx=OFF',
                    '--max-connections=24', '--log-error-verbosity=3'],
                    stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            while True:
                assert process.poll() is None, 'Oracle MySQL exited before accepting SQL'
                try:
                    if address.exists() and sql('SELECT 42') == '42':
                        break
                except AssertionError:
                    pass
                assert time.monotonic() - begin < 90, 'Oracle MySQL readiness timeout'
                time.sleep(.05)
            timings[label] = (time.monotonic() - begin) * 1000

        def stop():
            nonlocal process
            begin = time.monotonic()
            process.send_signal(signal.SIGTERM)
            assert process.wait(timeout=args.shutdown_timeout) == 0
            timings.setdefault('shutdown_ms', []).append((time.monotonic() - begin) * 1000)
            process = None

        try:
            begin = time.monotonic()
            command([*options, '--initialize-insecure'], timeout=120)
            timings['initialize_ms'] = (time.monotonic() - begin) * 1000
            print('ORACLE_MYSQL_INITIALIZED', flush=True)
            start('startup_ms')
            sql('CREATE DATABASE probe; CREATE TABLE probe.values_table '
                '(id INT PRIMARY KEY, value BIGINT) ENGINE=InnoDB; '
                'START TRANSACTION; INSERT INTO probe.values_table VALUES (1,42); COMMIT; '
                'START TRANSACTION; INSERT INTO probe.values_table VALUES (2,99); ROLLBACK;')
            assert sql('SELECT SUM(value) FROM probe.values_table') == '42'
            begin = time.monotonic()
            def write(index):
                sql(f'INSERT INTO probe.values_table VALUES ({index + 10},{index})')
            with ThreadPoolExecutor(4) as clients:
                list(clients.map(write, range(32)))
            timings['concurrent_32_transactions_ms'] = (time.monotonic() - begin) * 1000
            assert sql('SELECT COUNT(*),SUM(value) FROM probe.values_table') == '33\t538'
            print('ORACLE_MYSQL_TRANSACTIONS_OK', flush=True)
            stop()
            start('restart_ms')
            assert sql('SELECT COUNT(*),SUM(value) FROM probe.values_table') == '33\t538'
            sql('INSERT INTO probe.values_table VALUES (100,1234)')
            process.kill()
            assert process.wait(timeout=30) != 0
            process = None
            start('crash_recovery_ms')
            assert sql('SELECT COUNT(*),SUM(value) FROM probe.values_table') == '34\t1772'
            stop()
            print(json.dumps(timings), flush=True)
            print('ORACLE_MYSQL_TRANSACTIONS_CRASH_RECOVERY_OK', flush=True)
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            if log_path.exists():
                print(log_path.read_text(errors='replace')[-16000:], flush=True)


if __name__ == '__main__':
    main()
