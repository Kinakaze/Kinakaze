"""Real tool error paths, Unicode filenames, contention and binary round trips."""
import hashlib
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tarfile
import tempfile


def run(*command, data=None, status=0):
    result = subprocess.run(command, input=data, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=25)
    if status is None:
        assert result.returncode > 0, (command, result.returncode, result.stderr[-2000:])
    else:
        assert result.returncode == status, (command, result.returncode, result.stderr[-2000:])
    return result


def sqlite_boundary():
    db = str(Path('数据库 with spaces.db').resolve())
    first = sqlite3.connect(db)
    assert first.execute('pragma journal_mode=wal').fetchone()[0] == 'wal'
    first.execute('create table t(id primary key, body blob)')
    payload = bytes(range(256)) * 4096
    first.execute('insert into t values(1, ?)', (payload,))
    first.commit()
    first.execute('begin immediate')
    locked = run('/usr/bin/sqlite3', '-cmd', '.timeout 100', db,
                 'insert into t values(2, X\'00FF\');', status=5)
    assert b'locked' in locked.stderr, locked.stderr
    # WAL readers must keep working while another connection owns a write lock.
    assert run('/usr/bin/sqlite3', db, 'select length(body) from t where id=1;').stdout.strip() == b'1048576'
    first.rollback()
    run('/usr/bin/sqlite3', db, 'insert into t values(2, X\'00FF\');')
    assert first.execute('select body from t where id=1').fetchone()[0] == payload
    assert first.execute('select body from t where id=2').fetchone()[0] == b'\x00\xff'
    first.close()
    run('/usr/bin/sqlite3', 'file:' + db + '?mode=ro', 'delete from t;', status=None)
    assert run('/usr/bin/sqlite3', db, 'pragma integrity_check; select count(*) from t;').stdout == b'ok\n2\n'


def git_boundary():
    run('/usr/bin/git', 'init', '-q', 'origin')
    os.chdir('origin')
    run('/usr/bin/git', 'config', 'user.email', 'boundary@example.invalid')
    run('/usr/bin/git', 'config', 'user.name', 'Boundary fixture')
    Path('中文 empty.txt').write_bytes(b'')
    payload = bytes(range(256)) * 4096
    Path('binary payload.dat').write_bytes(payload)
    Path('conflict.txt').write_text('base\n')
    run('/usr/bin/git', 'add', '.')
    run('/usr/bin/git', 'commit', '-qm', 'fixture')
    run('/usr/bin/git', 'branch', 'side')
    Path('conflict.txt').write_text('main\n')
    run('/usr/bin/git', 'commit', '-qam', 'main')
    main = run('/usr/bin/git', 'rev-parse', 'HEAD').stdout.strip()
    run('/usr/bin/git', 'checkout', '-q', 'side')
    Path('conflict.txt').write_text('side\n')
    run('/usr/bin/git', 'commit', '-qam', 'side')
    run('/usr/bin/git', 'merge', main.decode(), status=1)
    assert b'<<<<<<<' in Path('conflict.txt').read_bytes()
    run('/usr/bin/git', 'merge', '--abort')
    assert Path('conflict.txt').read_bytes() == b'side\n'
    run('/usr/bin/git', 'fsck', '--full')
    os.chdir('..')
    run('/usr/bin/git', 'clone', '-q', '--no-local', 'origin', 'clone')
    assert Path('clone/中文 empty.txt').read_bytes() == b''
    assert Path('clone/binary payload.dat').read_bytes() == payload
    run('/usr/bin/git', '-C', 'clone', 'archive', '-o', str(Path('export.tar').resolve()), 'HEAD')
    with tarfile.open('export.tar') as archive:
        assert archive.extractfile('binary payload.dat').read() == payload
        assert archive.extractfile('中文 empty.txt').read() == b''


def ffmpeg_boundary():
    base = ('/usr/bin/ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error')
    Path('broken.wav').write_bytes(b'RIFF\x10\x00\x00\x00WAVEbroken')
    invalid = run(*base, '-i', 'broken.wav', '-f', 'null', '-', status=1)
    assert invalid.stderr, 'missing invalid-input diagnostic'
    run(*base, '-i', 'missing file.wav', '-f', 'null', '-', status=1)
    raw = bytes((i * 47) % 256 for i in range(65 * 33 * 3 * 3))
    encoded = '中文 odd dimensions.mkv'
    run(*base, '-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', '65x33',
        '-framerate', '3', '-i', 'pipe:0', '-c:v', 'ffv1', '-level', '3',
        '-pix_fmt', 'bgr0', encoded, data=raw)
    decoded = run(*base, '-i', encoded, '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1').stdout
    assert decoded == raw, (len(decoded), len(raw))
    before = hashlib.sha256(Path(encoded).read_bytes()).digest()
    run(*base, '-n', '-i', encoded, encoded, status=1)
    assert hashlib.sha256(Path(encoded).read_bytes()).digest() == before


def compiler_boundary():
    Path('broken.c').write_text('int main( { this is not C; }\n')
    Path('good name.c').write_text('#include <stdio.h>\nint main(void){puts("BOUNDARY_COMPILE_OK");return 23;}\n')
    for tool in ('gcc', 'clang'):
        result = run('/usr/bin/' + tool, 'broken.c', '-o', 'broken-' + tool, status=None)
        assert b'error:' in result.stderr and not Path('broken-' + tool).exists()
        output = '中文 program-' + tool
        run('/usr/bin/' + tool, 'good name.c', '-o', output)
        assert run('./' + output, status=23).stdout == b'BOUNDARY_COMPILE_OK\n'
        run('/usr/bin/' + tool, 'good name.c', '-lkinakaze_missing_fixture', '-o', 'missing-' + tool, status=None)
        assert not Path('missing-' + tool).exists()


def node_boundary():
    script = r'''
const fs = require('fs'), cp = require('child_process'), assert = require('assert');
const { Worker } = require('worker_threads');
const data = Buffer.alloc(1024 * 1024); for (let i = 0; i < data.length; i++) data[i] = i % 256;
fs.writeFileSync('中文 data.bin', data); assert(fs.readFileSync('中文 data.bin').equals(data));
const child = cp.spawnSync(process.execPath, ['-e',
  "process.stdout.write(Buffer.alloc(262144,65));process.stderr.write(Buffer.alloc(262144,66));process.exitCode=23"],
  {maxBuffer: 2 * 1024 * 1024, timeout: 15000});
assert.strictEqual(child.status, 23); assert(child.stdout.equals(Buffer.alloc(262144,65)));
assert(child.stderr.equals(Buffer.alloc(262144,66)));
const missing = cp.spawnSync('/does-not-exist-kinakaze-boundary', []);
assert.strictEqual(missing.error.code, 'ENOENT');
Promise.all(Array.from({length:4}, (_,i) => new Promise((resolve,reject) => {
  const worker = new Worker("const {parentPort,workerData}=require('worker_threads');parentPort.postMessage(workerData*workerData)",
    {eval:true,workerData:i});
  worker.once('message', v => {try {assert.strictEqual(v,i*i);resolve();} catch(e) {reject(e);}});
  worker.once('error', reject);
}))).then(() => console.log('NODE_BOUNDARY_CHILD_WORKERS_OK')).catch(e=>{console.error(e);process.exitCode=1;});
'''
    assert b'NODE_BOUNDARY_CHILD_WORKERS_OK' in run('/usr/bin/node', '-e', script).stdout


def compression_boundary(tool):
    payload = bytes(range(256)) * 4096
    name = '中文 data with spaces.bin'
    Path(name).write_bytes(payload)
    packed = run(tool, '-c', name).stdout
    assert run(tool, '-dc', data=packed).stdout == payload
    rejected = run(tool, '-dc', data=packed[:len(packed)//2], status=None)
    assert rejected.stderr, tool


def archive_boundary():
    compression_boundary('/bin/gzip')
    name = '中文 data with spaces.bin'
    payload = Path(name).read_bytes()
    Path('zero').write_bytes(b'')
    run('/bin/tar', 'czf', 'archive.tar.gz', name, 'zero')
    Path('out').mkdir()
    run('/bin/tar', 'xzf', 'archive.tar.gz', '-C', 'out')
    assert Path('out', name).read_bytes() == payload and Path('out/zero').read_bytes() == b''


selected = sys.argv[1]
probes = dict(sqlite=sqlite_boundary, git=git_boundary, ffmpeg=ffmpeg_boundary,
              compiler=compiler_boundary, node=node_boundary, archive=archive_boundary,
              bzip2=lambda: compression_boundary('/bin/bzip2'),
              xz=lambda: compression_boundary('/usr/bin/xz'))
with tempfile.TemporaryDirectory(prefix='kinakaze-software-boundary-') as directory:
    old = os.getcwd()
    try:
        os.chdir(directory)
        probes[selected]()
        print('SOFTWARE_BOUNDARY_' + selected.upper() + '_OK', flush=True)
    finally:
        os.chdir(old)
