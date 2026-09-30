"""Real Codex CLI shell/edit/test loop with a deterministic local Responses server."""
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
    parser.add_argument('--codex', required=True)
    args = parser.parse_args()
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    guest = '/tmp/codex-workflow-' + uuid.uuid4().hex
    fixture = root / guest.lstrip('/')
    (fixture / 'home/.codex').mkdir(parents=True)
    original = 'def total(values):\n    return sum(values[:-1])\n'
    tests = ('import unittest\nfrom daily import total\nclass Tests(unittest.TestCase):\n'
             '    def test_empty(self): self.assertEqual(total([]), 0)\n'
             '    def test_one(self): self.assertEqual(total([7]), 7)\n'
             '    def test_many(self): self.assertEqual(total([2, 3, 4]), 9)\n')
    (fixture / 'daily.py').write_text(original, encoding='utf-8')
    (fixture / 'test_daily.py').write_text(tests, encoding='utf-8')
    actions = ['cat daily.py', 'python3 -m unittest -v',
               'python3 -c "from pathlib import Path; source=Path(\'daily.py\'); '
               'source.write_text(source.read_text().replace(\'values[:-1]\',\'values\'))"',
               'python3 -m unittest -v']
    requests, errors = [], []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not self.path.endswith('/responses') or not 0 < length < 8 * 1024 * 1024:
                    self.send_error(400)
                    return
                request = json.loads(self.rfile.read(length))
                index = len(requests)
                requests.append(request)
                if index > len(actions):
                    raise ValueError('unexpected extra request')
                if index < len(actions):
                    candidates = [tool for tool in request.get('tools', [])
                                  if tool.get('type') == 'function' and tool.get('name') in
                                  ('exec_command', 'shell_command', 'shell')]
                    if not candidates:
                        raise ValueError('no supported shell tool: ' + str([tool.get('name') for tool in request.get('tools', [])]))
                    tool = candidates[0]
                    properties = tool['parameters']['properties']
                    if 'cmd' in properties:
                        arguments = dict(cmd=actions[index])
                        if 'yield_time_ms' in properties:
                            arguments['yield_time_ms'] = 10000
                    else:
                        command = properties['command']
                        arguments = dict(command=['/bin/bash', '-c', actions[index]]
                                         if command.get('type') == 'array' else actions[index])
                    if 'workdir' in properties:
                        arguments['workdir'] = guest
                    item = dict(id=f'item-{index}', type='function_call', call_id=f'call-{index}',
                                name=tool['name'], arguments=json.dumps(arguments), status='completed')
                else:
                    item = dict(id=f'item-{index}', type='message', role='assistant', status='completed',
                                content=[dict(type='output_text', text='CODEX_WORKFLOW_DONE', annotations=[])])
                response = dict(id=f'response-{index}', object='response', created_at=0,
                                status='completed', output=[item], model=request.get('model'),
                                usage=dict(input_tokens=1, output_tokens=1, total_tokens=2))
                events = [
                    dict(type='response.created', response={**response, 'status': 'in_progress', 'output': []}),
                    dict(type='response.output_item.added', output_index=0, item=item),
                    dict(type='response.output_item.done', output_index=0, item=item),
                    dict(type='response.completed', response=response),
                ]
                body = ''.join(f'event: {event["type"]}\ndata: {json.dumps(event)}\n\n' for event in events).encode()
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
    report = dict(scope=__doc__, root=str(root), dist=str(dist), guest=guest, passed=False,
                  sha256=distribution_hashes(dist),
                  cli_sha256=hashlib.sha256((root / args.codex.lstrip('/')).read_bytes()).hexdigest())
    environment = [f'HOME={guest}/home', f'CODEX_HOME={guest}/home/.codex',
                   'PATH=/usr/bin:/bin', 'LANG=C.UTF-8', 'TERM=dumb', 'OPENAI_API_KEY=local-fixture-only']
    config = ['model_provider="fixture"', 'model_providers.fixture.name="Fixture"',
              f'model_providers.fixture.base_url="http://127.0.0.1:{server.server_port}/v1"',
              'model_providers.fixture.wire_api="responses"', 'model_providers.fixture.env_key="OPENAI_API_KEY"',
              'model_providers.fixture.supports_websockets=false', 'model_providers.fixture.request_max_retries=0']
    command = ['/usr/bin/env', '-i', *environment, args.codex, 'exec', '--skip-git-repo-check',
               '--ephemeral', '--dangerously-bypass-approvals-and-sandbox', '--json', '--model', 'gpt-5.4']
    for setting in config:
        command.extend(['-c', setting])
    command.append('Read daily.py, run the tests, fix total, and rerun tests. Do not change tests.')
    try:
        with InitPool(root, dist, output / 'session', timeout=180) as pool:
            report['baseline'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest, expected_exit=1)
            report['agent'] = pool.run(command, cwd=guest, expect=['CODEX_WORKFLOW_DONE'])
            report['verification'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest)
        returned = []
        for index in range(len(actions)):
            items = [item for item in requests[index + 1].get('input', [])
                     if isinstance(item, dict) and item.get('type') == 'function_call_output'
                     and item.get('call_id') == f'call-{index}'] if len(requests) > index + 1 else []
            returned.append(items[-1].get('output', '') if items else '')
        report['tool_results'] = returned
        report['tool_results_validated'] = ('sum(values[:-1])' in json.dumps(returned[0])
            and 'FAILED (failures=2)' in json.dumps(returned[1])
            and 'Ran 3 tests' in json.dumps(returned[3]) and 'OK' in json.dumps(returned[3]))
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
