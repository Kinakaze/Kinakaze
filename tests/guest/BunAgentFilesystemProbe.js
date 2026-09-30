import assert from 'node:assert/strict';
import fs from 'node:fs';
import promises from 'node:fs/promises';
import path from 'node:path';

const directory = fs.mkdtempSync('/tmp/bun-agent-fs-');
try {
    const nested = path.join(directory, '-tmp-project', 'tasks');
    fs.mkdirSync(nested, { recursive: true, mode: 0o700 });
    assert.equal(fs.statSync(nested).isDirectory(), true);
    const source = 'def total(values):\n    return sum(values[:-1])\n';
    const filename = path.join(nested, 'daily.py');
    fs.writeFileSync(filename, source);
    const text = fs.readFileSync(filename, 'utf8');
    assert.equal(text, source);
    assert.equal(text.includes('sum(values[:-1])'), true);
    fs.writeFileSync(filename, text.replace('sum(values[:-1])', 'sum(values)'));
    assert.equal(fs.readFileSync(filename, 'utf8'), 'def total(values):\n    return sum(values)\n');
    const asynchronous = path.join(directory, '-tmp-async-project', 'tasks');
    await promises.mkdir(asynchronous, { recursive: true, mode: 0o700 });
    const asyncFile = path.join(asynchronous, 'daily.py');
    await promises.writeFile(asyncFile, source);
    assert.equal(await promises.readFile(asyncFile, 'utf8'), source);
    console.log('BUN_AGENT_FILESYSTEM_OK');
} finally {
    fs.rmSync(directory, { recursive: true });
}
