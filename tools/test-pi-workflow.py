"""Exercise the real pi CLI and built-in tools with a deterministic local provider.

This is a protocol/tool compatibility test, not an online model evaluation.
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import uuid

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('root', 'dist', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    parser.add_argument('--node', default='/root/node-v22.23.3-linux-x64/bin/node')
    parser.add_argument('--pi', default='/root/pi-test/node_modules/@mariozechner/pi-coding-agent/dist/cli.js')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    guest = '/root/pi-workflow-' + uuid.uuid4().hex
    fixture = args.root / guest.lstrip('/')
    fixture.mkdir(parents=True)
    original = 'def total(values):\n    return sum(values[:-1])\n'
    tests = ('import unittest\nfrom daily import total\nclass Tests(unittest.TestCase):\n'
             '    def test_empty(self): self.assertEqual(total([]), 0)\n'
             '    def test_one(self): self.assertEqual(total([7]), 7)\n'
             '    def test_many(self): self.assertEqual(total([2, 3, 4]), 9)\n')
    (fixture / 'daily.py').write_text(original)
    (fixture / 'test_daily.py').write_text(tests)
    requests = []
    actions = [
        ('read', dict(path='daily.py')),
        ('bash', dict(command='python3 -m unittest -v')),
        ('edit', dict(path='daily.py', oldText='sum(values[:-1])', newText='sum(values)')),
        ('bash', dict(command='python3 -m unittest -v')),
    ]

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            length = int(self.headers.get('Content-Length', '0'))
            if not self.path.endswith('/messages') or not 0 < length < 4*1024*1024:
                self.send_error(400)
                return
            request = json.loads(self.rfile.read(length))
            index = len(requests)
            requests.append(request)
            if index > len(actions):
                self.send_error(400, 'unexpected extra request')
                return
            tool = index < len(actions)
            if tool:
                name, arguments = actions[index]
                block = dict(type='tool_use', id=f'fixture-tool-{index}', name=name, input={})
                delta = dict(type='input_json_delta', partial_json=json.dumps(arguments))
            else:
                block = dict(type='text', text='')
                delta = dict(type='text_delta', text='PI_WORKFLOW_DONE')
            events = [
                ('message_start', dict(type='message_start', message=dict(id=f'fixture-{index}',
                    type='message', role='assistant', model='pi-fixture', content=[],
                    stop_reason=None, stop_sequence=None, usage=dict(input_tokens=1, output_tokens=0)))),
                ('content_block_start', dict(type='content_block_start', index=0, content_block=block)),
                ('content_block_delta', dict(type='content_block_delta', index=0, delta=delta)),
                ('content_block_stop', dict(type='content_block_stop', index=0)),
                ('message_delta', dict(type='message_delta', delta=dict(stop_reason='tool_use' if tool else 'end_turn',
                    stop_sequence=None), usage=dict(output_tokens=1))),
                ('message_stop', dict(type='message_stop')),
            ]
            body = ''.join(f'event: {name}\ndata: {json.dumps(value)}\n\n' for name, value in events).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    config = fixture / 'home/.pi/agent'
    config.mkdir(parents=True)
    (config / 'models.json').write_text(json.dumps(dict(providers=dict(compatibility=dict(
        baseUrl=f'http://127.0.0.1:{server.server_port}/v1', api='anthropic-messages',
        apiKey='local-fixture-only', models=[dict(id='pi-fixture', contextWindow=200000, maxTokens=4096)])))))
    report = dict(scope=__doc__.strip(), guest=guest, requests=0,
                  sha256=distribution_hashes(args.dist.resolve()), passed=False)
    environment = [f'HOME={guest}/home', f'PI_CODING_AGENT_DIR={guest}/home/.pi/agent',
                   f'PATH={str(Path(args.node).parent).replace(chr(92), "/")}:/usr/bin:/bin', 'LANG=C.UTF-8']
    try:
        with InitPool(args.root, args.dist, args.output / 'session', timeout=180) as pool:
            report['baseline'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest, expected_exit=1)
            report['pi'] = pool.run([args.node, args.pi, '--offline', '--no-session', '--no-extensions',
                '--no-skills', '--no-context-files', '--provider', 'compatibility', '--model', 'pi-fixture',
                '--print', 'Read daily.py, run the tests, fix total, and run tests again. Do not modify tests.'],
                cwd=guest, environment=environment, expect=['PI_WORKFLOW_DONE'])
            report['verification'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest)
        report['requests'] = len(requests)
        tool_results = []
        for index in range(len(actions)):
            blocks = [block for message in requests[index+1]['messages']
                      if isinstance(message.get('content'), list) for block in message['content']
                      if block.get('type') == 'tool_result' and block.get('tool_use_id') == f'fixture-tool-{index}'] if len(requests) > index+1 else []
            tool_results.append(blocks[-1] if blocks else {})
        report['tool_results_validated'] = (
            original.strip() in tool_results[0].get('content', '').replace('\r\n', '\n')
            and [result.get('is_error') for result in tool_results] == [False, True, False, False]
            and 'FAILED (failures=2)' in tool_results[1].get('content', '')
            and 'OK' in tool_results[3].get('content', ''))
        report['fixture_preserved'] = (fixture / 'test_daily.py').read_text() == tests
        report['edit_correct'] = (fixture / 'daily.py').read_text() == original.replace('values[:-1]', 'values')
        report['passed'] = (len(requests) == 5 and report['tool_results_validated'] and report['fixture_preserved'] and report['edit_correct']
            and all(report[name]['status'] == 'passed' for name in ('baseline', 'pi', 'verification')))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        (args.output / 'requests.json').write_text(json.dumps(requests, indent=2))
        (args.output / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({key: report[key] for key in ('guest', 'requests', 'passed')}))
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
