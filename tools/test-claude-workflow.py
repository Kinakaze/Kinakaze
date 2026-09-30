"""Test the real Claude CLI with a local deterministic tool-use provider.

Exercises tool transport and execution, not online model quality or authentication.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import uuid

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--claude', required=True, help='absolute guest executable path')
    args = parser.parse_args()
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    guest = '/tmp/claude-workflow-' + uuid.uuid4().hex
    fixture = root / guest.lstrip('/')
    home = fixture / 'home'
    home.mkdir(parents=True)
    original = 'def total(values):\n    return sum(values[:-1])\n'
    tests = ('import unittest\nfrom daily import total\nclass Tests(unittest.TestCase):\n'
             '    def test_empty(self): self.assertEqual(total([]), 0)\n'
             '    def test_one(self): self.assertEqual(total([7]), 7)\n'
             '    def test_many(self): self.assertEqual(total([2, 3, 4]), 9)\n')
    (fixture / 'daily.py').write_text(original, encoding='utf-8')
    (fixture / 'test_daily.py').write_text(tests, encoding='utf-8')
    actions = [
        ('Read', dict(file_path=guest + '/daily.py')),
        ('Bash', dict(command='python3 -m unittest -v', description='Run fixture tests')),
        ('Edit', dict(file_path=guest + '/daily.py', old_string='sum(values[:-1])', new_string='sum(values)')),
        ('Bash', dict(command='python3 -m unittest -v', description='Verify corrected fixture')),
    ]
    requests = []
    errors = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not self.path.startswith('/v1/messages') or not 0 < length < 8 * 1024 * 1024:
                    self.send_error(400)
                    return
                request = json.loads(self.rfile.read(length))
                if self.path.startswith('/v1/messages/count_tokens'):
                    body = b'{"input_tokens":100}'
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                index = len(requests)
                requests.append(request)
                if index > len(actions):
                    raise ValueError('unexpected extra model request')
                if index < len(actions):
                    name, arguments = actions[index]
                    if name not in {tool['name'] for tool in request.get('tools', [])}:
                        raise ValueError('required tool not advertised: ' + name)
                    block = dict(type='tool_use', id=f'fixture-{index}', name=name, input={})
                    delta = dict(type='input_json_delta', partial_json=json.dumps(arguments))
                else:
                    block = dict(type='text', text='')
                    delta = dict(type='text_delta', text='CLAUDE_WORKFLOW_DONE')
                events = [
                    ('message_start', dict(type='message_start', message=dict(id=f'message-{index}',
                        type='message', role='assistant', model=request.get('model'), content=[],
                        stop_reason=None, stop_sequence=None, usage=dict(input_tokens=1, output_tokens=0)))),
                    ('content_block_start', dict(type='content_block_start', index=0, content_block=block)),
                    ('content_block_delta', dict(type='content_block_delta', index=0, delta=delta)),
                    ('content_block_stop', dict(type='content_block_stop', index=0)),
                    ('message_delta', dict(type='message_delta', delta=dict(
                        stop_reason='tool_use' if index < len(actions) else 'end_turn', stop_sequence=None),
                        usage=dict(output_tokens=1))),
                    ('message_stop', dict(type='message_stop')),
                ]
                body = ''.join(f'event: {name}\ndata: {json.dumps(value)}\n\n' for name, value in events).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except Exception as error:
                errors.append(str(error))
                self.send_error(500)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    report = dict(scope=__doc__, root=str(root), dist=str(dist), guest=guest,
                  sha256=distribution_hashes(dist), passed=False,
                  cli_sha256=hashlib.sha256((root / args.claude.lstrip('/')).read_bytes()).hexdigest())
    environment = [f'HOME={guest}/home', f'CLAUDE_CONFIG_DIR={guest}/home/.claude',
                   'PATH=/usr/bin:/bin', 'LANG=C.UTF-8', 'TERM=dumb',
                   f'ANTHROPIC_BASE_URL=http://127.0.0.1:{server.server_port}',
                   'ANTHROPIC_API_KEY=local-fixture-only',
                   'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1', 'DISABLE_AUTOUPDATER=1']
    try:
        with InitPool(root, dist, output / 'session', timeout=180) as pool:
            report['baseline'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'],
                                          cwd=guest, expected_exit=1)
            report['agent'] = pool.run(['/usr/bin/env', '-i', *environment, args.claude,
                '--bare', '--disable-slash-commands', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}',
                '--no-session-persistence', '--allowedTools', 'Read,Edit,Bash', '--model', 'claude-sonnet-4-6',
                '--output-format', 'stream-json', '--verbose', '-p',
                'Read daily.py, run tests, fix total, and run tests again. Do not change tests.'],
                cwd=guest, expect=['CLAUDE_WORKFLOW_DONE'])
            report['verification'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest)
        returned = []
        for index in range(len(actions)):
            blocks = [block for message in requests[index + 1].get('messages', [])
                      if isinstance(message.get('content'), list) for block in message['content']
                      if block.get('type') == 'tool_result' and block.get('tool_use_id') == f'fixture-{index}'] if len(requests) > index + 1 else []
            returned.append(blocks[-1] if blocks else {})
        report['tool_results'] = returned
        report['tool_results_validated'] = (
            'sum(values[:-1])' in json.dumps(returned[0])
            and 'FAILED (failures=2)' in json.dumps(returned[1])
            and 'Ran 3 tests' in json.dumps(returned[3]) and 'OK' in json.dumps(returned[3])
            and not any(returned[index].get('is_error') for index in (0, 2, 3)))
        report['edit_correct'] = (fixture / 'daily.py').read_text() == original.replace('values[:-1]', 'values')
        report['tests_preserved'] = (fixture / 'test_daily.py').read_text() == tests
        report['passed'] = (len(requests) == 5 and not errors and report['tool_results_validated']
                            and report['edit_correct'] and report['tests_preserved']
                            and all(report[name]['status'] == 'passed' for name in ('baseline', 'agent', 'verification')))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        report.update(requests=len(requests), provider_errors=errors)
        (output / 'requests.json').write_text(json.dumps(requests, indent=2), encoding='utf-8')
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(dict(passed=report['passed'], requests=len(requests), provider_errors=errors)))
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
