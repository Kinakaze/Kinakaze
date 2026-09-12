"""Compare SQLite rollback/WAL readers and writers in separate real processes."""
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

CHILD = '''
import json, sqlite3, sys
connection = None
try:
    connection = sqlite3.connect(sys.argv[1], timeout=.1)
    if sys.argv[2] == 'read':
        rows = connection.execute('select value from t order by value').fetchall()
    else:
        connection.execute('insert into t values (2)')
        connection.commit()
        rows = []
    print(json.dumps(dict(status='ok', rows=rows)))
except sqlite3.Error as error:
    print(json.dumps(dict(status='error', message=str(error),
                         code=getattr(error, 'sqlite_errorcode', None),
                         name=getattr(error, 'sqlite_errorname', None))))
finally:
    if connection is not None:
        connection.close()
'''


def main():
    results = []
    with tempfile.TemporaryDirectory(prefix='kinakaze-sqlite-process-') as directory:
        for mode in ('delete', 'wal'):
            for unicode_name in (False, True):
                for locked in (False, True):
                    for action in ('read', 'write'):
                        case = dict(mode=mode, unicode_name=unicode_name, locked=locked, action=action)
                        name = ('数据库 ' if unicode_name else 'ascii-') + str(len(results)) + '.db'
                        path = str(Path(directory) / name)
                        connection = sqlite3.connect(path)
                        try:
                            assert connection.execute('pragma journal_mode=' + mode).fetchone()[0] == mode
                            connection.execute('create table t(value integer)')
                            connection.execute('insert into t values(1)')
                            connection.commit()
                            if locked:
                                connection.execute('begin immediate')
                            result = subprocess.run([sys.executable, '-c', CHILD, path, action],
                                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
                            case['exit_code'] = result.returncode
                            case['stderr'] = result.stderr.decode('utf-8', errors='replace')
                            try:
                                case['actual'] = json.loads(result.stdout)
                            except (ValueError, UnicodeError):
                                case['actual'] = dict(status='invalid-output', output=repr(result.stdout))
                            expected_busy = locked and action == 'write'
                            case['expected'] = 'SQLITE_BUSY' if expected_busy else 'ok'
                            actual = case['actual']
                            case['passed'] = result.returncode == 0 and (
                                (actual.get('code') == 5 and actual.get('status') == 'error') if expected_busy else
                                (actual.get('status') == 'ok' and (action != 'read' or actual.get('rows') == [[1]])))
                            connection.rollback()
                            case['integrity'] = connection.execute('pragma integrity_check').fetchone()[0]
                            case['passed'] = case['passed'] and case['integrity'] == 'ok'
                        finally:
                            connection.close()
                        results.append(case)
                        print(json.dumps(case), flush=True)
    report = dict(python=sys.version, sqlite=sqlite3.sqlite_version, platform=sys.platform,
                  results=results, passed=sum(c['passed'] for c in results), total=len(results))
    print(json.dumps(report), flush=True)
    return int(report['passed'] != report['total'])


if __name__ == '__main__':
    raise SystemExit(main())
