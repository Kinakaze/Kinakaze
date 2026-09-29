"""Real InnoDB transactions, concurrent clients and recovery after restart.

Uses a disposable datadir and Unix socket; never opens a TCP listener or touches
the distribution's database. Preserve the server log on failure.
"""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


with tempfile.TemporaryDirectory(prefix='kinakaze-mariadb-') as directory:
    root = Path(directory)
    data, address, logfile = root / 'data', root / 'mysql.sock', root / 'server.log'
    timings = {}
    process = None

    def command(argv, timeout=90):
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=timeout)
        assert result.returncode == 0, (argv, result.returncode,
            result.stdout.decode(errors='replace'), result.stderr.decode(errors='replace'))
        return result.stdout.decode().strip()

    def sql(query):
        return command(['/usr/bin/mariadb', '--no-defaults', '--protocol=socket',
                        '--socket=' + str(address), '--user=root', '--batch',
                        '--skip-column-names', '-e', query], timeout=30)

    def start(label):
        global process
        begin = time.monotonic()
        with logfile.open('ab') as log:
            process = subprocess.Popen(['/usr/sbin/mariadbd', '--no-defaults',
                '--user=' + str(os.getuid()), '--datadir=' + str(data),
                '--socket=' + str(address), '--pid-file=' + str(root / 'server.pid'),
                '--skip-networking', '--innodb-buffer-pool-size=64M',
                '--max-connections=24'], stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        while True:
            assert process.poll() is None, 'MariaDB exited before accepting SQL'
            try:
                if address.exists() and sql('SELECT 42') == '42':
                    break
            except AssertionError:
                pass
            assert time.monotonic() - begin < 60, 'MariaDB readiness timeout'
            time.sleep(.05)
        timings[label] = (time.monotonic() - begin) * 1000

    def stop():
        global process
        process.send_signal(signal.SIGTERM)
        assert process.wait(timeout=30) == 0
        process = None

    try:
        command(['/usr/bin/mariadb-install-db', '--no-defaults', '--datadir=' + str(data),
                 '--auth-root-authentication-method=normal', '--skip-test-db'])
        start('startup_ms')
        assert 'MariaDB' in sql('SELECT VERSION()')
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
        assert sql('SELECT COUNT(*),SUM(value) FROM probe.values_table') == '33\t538'
        timings['concurrent_32_transactions_ms'] = (time.monotonic() - begin) * 1000
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
        print('MARIADB_INNODB_TRANSACTIONS_RESTART_OK', flush=True)
        print('MARIADB_INNODB_CRASH_RECOVERY_OK', flush=True)
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        if logfile.exists():
            print(logfile.read_text(errors='replace')[-16000:], flush=True)
