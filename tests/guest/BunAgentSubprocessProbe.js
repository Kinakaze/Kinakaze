import assert from 'node:assert/strict';
import fs from 'node:fs';

const directory = fs.mkdtempSync('/tmp/bun-agent-spawn-');
try {
    const cwd = directory + '/目录 space';
    fs.mkdirSync(cwd);
    async function run(index) {
        const token = `中文-${index}`;
        const child = Bun.spawn(['/bin/bash', '-c',
            'read -r line; printf "OUT:%s\\n" "$line"; printf "ERR:%s\\n" "$line" >&2; ' +
            'python3 -c \'import os; assert os.path.basename(os.getcwd())=="目录 space"; print("CHILD_OK")\''],
            { cwd, stdin: 'pipe', stdout: 'pipe', stderr: 'pipe', env: { ...process.env, PATH: '/usr/bin:/bin' } });
        const deadline = setTimeout(() => child.kill('SIGKILL'), 15000);
        try {
            child.stdin.write(token + '\n');
            child.stdin.end();
            const [stdout, stderr, status] = await Promise.all([
                new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
            assert.equal(status, 0, JSON.stringify({ index, stdout, stderr, status }));
            assert.equal(stdout, `OUT:${token}\nCHILD_OK\n`);
            assert.equal(stderr, `ERR:${token}\n`);
        } finally {
            clearTimeout(deadline);
        }
    }
    // Four independent streams repeatedly create, replace and close descriptors.
    await Promise.all(Array.from({ length: 4 }, async (_, worker) => {
        for (let index = worker; index < 32; index += 4) await run(index);
    }));
    console.log('BUN_AGENT_SUBPROCESS_STREAMS_CWD_OK');
} finally {
    fs.rmSync(directory, { recursive: true });
}
