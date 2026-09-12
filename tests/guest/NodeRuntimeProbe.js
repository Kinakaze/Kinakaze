// Run inside the Linux guest; exercise the APIs used by agent command-line tools.
'use strict';
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs/promises');
const net = require('node:net');
const path = require('node:path');
const vm = require('node:vm');
const zlib = require('node:zlib');
const { promisify } = require('node:util');
const { once } = require('node:events');
const { execFile, execFileSync } = require('node:child_process');
const { Worker } = require('node:worker_threads');

async function echo(address, payload) {
    const server = net.createServer(socket => socket.pipe(socket));
    await new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(address, resolve);
    });
    try {
        const bound = server.address();
        const socket = typeof bound === 'string' ? net.connect(bound) :
            net.connect({ host: '127.0.0.1', port: bound.port });
        try {
            const chunks = [];
            socket.on('data', chunk => chunks.push(chunk));
            const ended = once(socket, 'end');
            await once(socket, 'connect');
            socket.end(payload);
            await ended;
            assert.deepEqual(Buffer.concat(chunks), payload);
        } finally {
            socket.destroy();
        }
    } finally {
        await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    }
}

async function main() {
    const payload = Buffer.alloc(2 * 1024 * 1024);
    for (let i = 0; i < payload.length; i++) payload[i] = (i * 17 + (i >>> 8)) & 255;
    const hash = crypto.createHash('sha256').update(payload).digest('hex');
    assert.deepEqual(await promisify(zlib.gunzip)(await promisify(zlib.gzip)(payload)), payload);
    const script = new vm.Script('function sum(n) { let v = 0; for(let i = 0; i < n; i++) v += i; return v; } sum(200000)');
    for (let i = 0; i < 25; i++) assert.equal(script.runInNewContext(), 19999900000);
    console.log('NODE_VM_ZLIB_OK');

    const shared = new SharedArrayBuffer(4);
    const worker = new Worker(`
        const { parentPort, workerData } = require('node:worker_threads');
        Atomics.add(new Int32Array(workerData), 0, 41);
        parentPort.postMessage(require('node:crypto').createHash('sha256').update('worker').digest('hex'));
    `, { eval: true, workerData: shared });
    const message = once(worker, 'message');
    const exited = once(worker, 'exit');
    assert.deepEqual(await message, [crypto.createHash('sha256').update('worker').digest('hex')]);
    assert.deepEqual(await exited, [0]);
    assert.equal(Atomics.add(new Int32Array(shared), 0, 1), 41);
    assert.equal(Atomics.load(new Int32Array(shared), 0), 42);
    console.log('NODE_THREADS_OK');

    const directory = await fs.mkdtemp('/tmp/node-agent-');
    try {
        const file = path.join(directory, 'payload');
        await fs.writeFile(file, payload);
        await fs.rename(file, file + '.renamed');
        assert.equal(crypto.createHash('sha256').update(await fs.readFile(file + '.renamed')).digest('hex'), hash);
        assert.equal((await fs.stat(file + '.renamed')).size, payload.length);
        assert.equal(execFileSync('/bin/busybox', ['echo', 'sync-child']).toString().trim(), 'sync-child');
        const result = await promisify(execFile)('/bin/sh', ['-c', 'printf "%s\n%s\n" "$PWD" "$AGENT_PROBE"'], {
            cwd: directory, env: { ...process.env, AGENT_PROBE: '中文' },
        });
        assert.equal(result.stdout, directory + '\n中文\n');
        await assert.rejects(promisify(execFile)('/bin/sh', ['-c', 'exit 23']), error => error.code === 23);
        console.log('NODE_FILES_CHILDREN_OK');
        await echo(path.join(directory, 'socket'), payload);
        await echo({ host: '127.0.0.1', port: 0 }, payload);
        console.log('NODE_SOCKETS_OK');
    } finally {
        await fs.rm(directory, { recursive: true, force: true });
    }
    console.log('NODE_RUNTIME_OK');
}

main().catch(error => { console.error(error); process.exitCode = 1; });
