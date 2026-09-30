"""Run an extra agent's real tools with a local model-protocol fixture.

Checks reads, failing tests, native edits, Unicode paths, shell output and final
tests. This is Linux tool/protocol compatibility, not an online model evaluation.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shlex
import threading
import uuid

from init_pool import InitPool, distribution_hashes
from agent_resource_watch import AgentResourceWatch


SOURCE = '# 求和 🧪\ndef total(values):\n    return sum(values[:-1])\n'
TESTS = ('import unittest\nfrom daily import total\nclass Tests(unittest.TestCase):\n'
         '    def test_empty(self): self.assertEqual(total([]), 0)\n'
         '    def test_one(self): self.assertEqual(total([7]), 7)\n'
         '    def test_many(self): self.assertEqual(total([2, 3, 4]), 9)\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--agent', choices=('opencode', 'zcode', 'dsh'), required=True)
    parser.add_argument('--cli', required=True)
    parser.add_argument('--node', default='/root/node-v22.23.3-linux-x64/bin/node')
    parser.add_argument('--timeout', type=float, default=180)
    parser.add_argument('--memory-limit-mb', type=int,
                        help='owned Job commit cap (default: OpenCode 24576, others 16384 MiB)')
    parser.add_argument('--trace', action='store_true', help='retain native libc diagnostics in session logs')
    args = parser.parse_args()
    if args.memory_limit_mb is None:
        args.memory_limit_mb = 24576 if args.agent == 'opencode' else 16384
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    if args.memory_limit_mb <= 0:
        parser.error('--memory-limit-mb must be positive')
    if args.trace:
        os.environ['KINAKAZE_TRACE'] = '1'
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    if output.exists():
        parser.error('--output must be new to preserve previous reports')
    if not args.cli.startswith('/') or not (root / args.cli.lstrip('/')).is_file():
        parser.error('--cli must be an installed absolute guest path')
    output.mkdir(parents=True)
    guest = '/tmp/openai-agent-tools-' + uuid.uuid4().hex
    fixture = root / guest.lstrip('/')
    for directory in ('home/.config', 'home/.cache', 'home/.local/share', 'home/.local/state'):
        (fixture / directory).mkdir(parents=True, exist_ok=True)
    (fixture / 'daily.py').write_bytes(SOURCE.encode())
    (fixture / 'test_daily.py').write_bytes(TESTS.encode())
    note = '目录 space/子目录/note.txt'
    actions = [
        ('read-source', 'read', dict(path='daily.py'), ['求和', 'sum(values[:-1])']),
        ('search-source', 'search', dict(pattern='values', path=guest), ['daily.py', 'values']),
        ('failing-tests', 'bash', dict(command='python3 -m unittest -v'), ['FAILED (failures=2)']),
        ('write-unicode', 'write', dict(path=note, content='你好 🧪\n'), []),
        ('read-unicode', 'read', dict(path=note), ['你好 🧪']),
        ('rejected-edit', 'edit', dict(path='daily.py', old='DOES_NOT_EXIST', new='invalid'),
         [dict(opencode='Could not find oldString', dsh='old_string was not found',
               zcode='DOES_NOT_EXIST')[args.agent]]),
        ('edit-source', 'edit', dict(path='daily.py', old='sum(values[:-1])', new='sum(values)'), []),
        ('edit-unicode', 'edit', dict(path=note, old='你好 🧪', new='已修改 ✅'), []),
        ('read-edited-unicode', 'read', dict(path=note), ['已修改 ✅']),
        ('passing-tests', 'bash', dict(command='python3 -m unittest -v'), ['Ran 3 tests', 'OK']),
        ('stderr-output', 'bash', dict(command="python3 -c 'import sys; print(\"STDERR_ONLY_OK\", file=sys.stderr)'"), ['STDERR_ONLY_OK']),
        ('large-output', 'bash', dict(command='python3 -c ' + shlex.quote('[print("line-%04d" % n) for n in range(1200)]; print("LARGE_OUTPUT_END")')), ['LARGE_OUTPUT_END']),
    ]
    requests, results, errors = [], [], []
    request_bytes = 0
    active_pool = None
    lock = threading.Lock()
    done = threading.Event()
    report = dict(scope=__doc__, agent=args.agent, root=str(root), dist=str(dist), guest=guest,
                  sha256=distribution_hashes(dist),
                  cli_sha256=hashlib.sha256((root / args.cli.lstrip('/')).read_bytes()).hexdigest(),
                  harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  planned=len(actions), timeout_seconds=args.timeout, memory_limit_mb=args.memory_limit_mb,
                  results=results, provider_errors=errors, passed=False)

    def persist():
        (output / 'requests.json').write_text(json.dumps(requests, indent=2, ensure_ascii=False), encoding='utf-8')
        (output / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')

    def tool_call(request, action):
        case, kind, arguments, _ = action
        advertised = {tool['function']['name']: tool['function'] for tool in request.get('tools', []) if tool.get('type') == 'function'}
        aliases = dict(read=('read', 'read_file'), write=('write', 'write_file'),
                       edit=('edit', 'edit_file'), bash=('bash', 'Bash', 'run_shell_command'),
                       search=('grep', 'Grep', 'search'))
        name = next((name for name in aliases[kind] if name in advertised), None)
        if name is None:
            raise ValueError(f'{case}: no {kind} tool in {list(advertised)}')
        schema = advertised[name].get('parameters', {})
        properties = schema.get('properties', {})
        payload = {}
        if 'i' in properties:
            payload['i'] = 'Verify ' + case

        def put(choices, value):
            key = next((key for key in choices if key in properties), None)
            if key is None:
                raise ValueError(f'{case}: unsupported schema for {name}: {schema}')
            payload[key] = value

        if kind in ('read', 'write', 'edit'):
            put(('filePath', 'path', 'file_path'), guest + '/' + arguments['path'])
        if kind == 'write':
            put(('content',), arguments['content'])
        elif kind == 'edit':
            put(('oldString', 'oldText', 'old_text', 'old_string', 'old'), arguments['old'])
            put(('newString', 'newText', 'new_text', 'new_string', 'new'), arguments['new'])
        elif kind == 'bash':
            put(('command',), arguments['command'])
            if 'description' in properties:
                payload['description'] = 'Run compatibility fixture ' + case
        elif kind == 'search':
            put(('pattern',), arguments['pattern'])
            if 'path' in properties:
                payload['path'] = arguments['path']
            if 'include' in properties:
                payload['include'] = '*.py'
        missing = [key for key in schema.get('required', []) if key not in payload]
        if missing:
            raise ValueError(f'{case}: missing required fields {missing}: {schema}')
        return name, payload

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            nonlocal request_bytes
            length = int(self.headers.get('Content-Length', '0'))
            endpoint = '/messages' if args.agent == 'dsh' else '/chat/completions'
            if not self.path.endswith(endpoint) or not 0 < length < 8 * 1024 * 1024:
                self.send_error(400, 'unexpected fixture endpoint or body size')
                return
            try:
                request = json.loads(self.rfile.read(length))
                with lock:
                    if len(requests) >= 64 or request_bytes + length > 32 * 1024 * 1024:
                        raise ValueError('fixture request recording limit exceeded')
                    request_bytes += length
                    requests.append(dict(path=self.path, body=request))
                    if args.agent == 'dsh':
                        request = dict(request, _messages_protocol=True,
                            tools=[dict(type='function', function=dict(name=tool['name'],
                                parameters=tool['input_schema'])) for tool in request.get('tools', [])],
                            messages=[dict(role='tool', tool_call_id=block['tool_use_id'],
                                content=block.get('content', '')) for message in request.get('messages', [])
                                if isinstance(message.get('content'), list) for block in message['content']
                                if block.get('type') == 'tool_result'])
                    if not request.get('tools'):
                        self.reply(request, text='Compatibility fixture')
                        persist()
                        return
                    index = len(results)
                    if index:
                        previous = f'fixture-{index - 1}'
                        matches = [message for message in request.get('messages', [])
                                   if message.get('role') == 'tool' and message.get('tool_call_id') == previous]
                        if not matches:
                            raise ValueError('missing actual tool result: ' + previous)
                    # Each response asks for exactly one tool. Validate the latest
                    # returned result before issuing the next action.
                    if getattr(self.server, 'pending', False):
                        index = len(results)
                        case, _, _, markers = actions[index]
                        call_id = f'fixture-{index}'
                        matches = [message for message in request.get('messages', [])
                                   if message.get('role') == 'tool' and message.get('tool_call_id') == call_id]
                        if not matches:
                            raise ValueError('missing actual tool result: ' + call_id)
                        value = matches[-1].get('content', '')
                        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
                        missing = [marker for marker in markers if marker not in text]
                        valid = not missing
                        if case == 'rejected-edit':
                            valid = valid and (fixture / 'daily.py').read_bytes() == SOURCE.encode()
                        elif case == 'edit-source':
                            valid = valid and (fixture / 'daily.py').read_bytes() == SOURCE.replace('values[:-1]', 'values').encode()
                        elif case == 'write-unicode':
                            valid = valid and (fixture / note).is_file() and (fixture / note).read_bytes() == '你好 🧪\n'.encode()
                        elif case == 'edit-unicode':
                            valid = valid and (fixture / note).read_bytes() == '已修改 ✅\n'.encode()
                        results.append(dict(case=case, passed=valid, missing=missing, output=value))
                        self.server.pending = False
                        persist()
                        print(json.dumps(dict(agent=args.agent, case=case, passed=valid)), flush=True)
                        if not valid:
                            raise ValueError(f'{case}: missing={missing} or fixture state incorrect')
                    index = len(results)
                    if index == len(actions):
                        self.reply(request, text='EXTRA_AGENT_WORKFLOW_DONE')
                        done.set()
                    else:
                        name, payload = tool_call(request, actions[index])
                        self.reply(request, call=dict(id=f'fixture-{index}', type='function', function=dict(name=name, arguments=json.dumps(payload, ensure_ascii=False))))
                        self.server.pending = True
                    persist()
            except Exception as error:
                with lock:
                    errors.append(str(error))
                    persist()
                self.send_error(400, 'fixture validation failed')
                if active_pool is not None:
                    active_pool.child.close()

        def reply(self, request, text=None, call=None):
            usage = dict(prompt_tokens=10, completion_tokens=10, total_tokens=20)
            if request.get('_messages_protocol'):
                block = (dict(type='tool_use', id=call['id'], name=call['function']['name'], input={})
                         if call else dict(type='text', text=''))
                delta = (dict(type='input_json_delta', partial_json=call['function']['arguments'])
                         if call else dict(type='text_delta', text=text or ''))
                events = [dict(type='message_start', message=dict(id='msg-fixture', type='message',
                    role='assistant', model=request.get('model'), content=[], stop_reason=None,
                    stop_sequence=None, usage=dict(input_tokens=10, output_tokens=0))),
                    dict(type='content_block_start', index=0, content_block=block),
                    dict(type='content_block_delta', index=0, delta=delta),
                    dict(type='content_block_stop', index=0),
                    dict(type='message_delta', delta=dict(stop_reason='tool_use' if call else 'end_turn',
                         stop_sequence=None), usage=dict(output_tokens=10)), dict(type='message_stop')]
                body = ''.join(f'event: {event["type"]}\ndata: {json.dumps(event, ensure_ascii=False)}\n\n'
                               for event in events).encode()
                content_type = 'text/event-stream'
            elif request.get('stream'):
                common = dict(id='chatcmpl-fixture', object='chat.completion.chunk', created=1, model='fixture')
                delta = dict(role='assistant', content=text or '')
                if call:
                    delta['tool_calls'] = [dict(index=0, **call)]
                chunks = [dict(**common, choices=[dict(index=0, delta=delta, finish_reason=None)]),
                          dict(**common, choices=[dict(index=0, delta={}, finish_reason='tool_calls' if call else 'stop')], usage=usage)]
                body = (''.join('data: ' + json.dumps(chunk, ensure_ascii=False) + '\n\n' for chunk in chunks) + 'data: [DONE]\n\n').encode()
                content_type = 'text/event-stream'
            else:
                message = dict(role='assistant', content=text or '')
                if call:
                    message['tool_calls'] = [call]
                body = json.dumps(dict(id='chatcmpl-fixture', object='chat.completion', created=1, model='fixture',
                    choices=[dict(index=0, message=message, finish_reason='tool_calls' if call else 'stop')], usage=usage), ensure_ascii=False).encode()
                content_type = 'application/json'
            self.send_response(200)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    base_url = f'http://127.0.0.1:{server.server_port}/v1'
    env = [f'HOME={guest}/home', f'XDG_CONFIG_HOME={guest}/home/.config',
           f'XDG_CACHE_HOME={guest}/home/.cache', f'XDG_DATA_HOME={guest}/home/.local/share',
           f'XDG_STATE_HOME={guest}/home/.local/state',
           f'PATH={str(Path(args.node).parent).replace(chr(92), "/")}:/usr/bin:/bin',
           'LANG=C.UTF-8', 'TERM=dumb', 'NO_COLOR=1']
    prompt = 'Exercise the fixture tools, fix daily.py, and verify its tests. Preserve test_daily.py.'
    if args.agent == 'opencode':
        config = dict(provider=dict(fixture=dict(npm='@ai-sdk/openai-compatible', name='Compatibility fixture',
            options=dict(baseURL=base_url, apiKey='local-fixture-only'),
            models=dict(fixture=dict(name='Fixture', limit=dict(context=200000, output=4096), tool_call=True)))),
            model='fixture/fixture', small_model='fixture/fixture', enabled_providers=['fixture'],
            permission='allow', share='disabled', autoupdate=False, plugin=[])
        (fixture / 'opencode.json').write_text(json.dumps(config, indent=2), encoding='utf-8')
        env.extend(['OPENCODE_DISABLE_AUTOUPDATE=1', 'OPENCODE_DISABLE_MODELS_FETCH=1',
                    f'OPENCODE_CONFIG={guest}/opencode.json'])
        command = [args.cli, 'run', '--format', 'json', '--model', 'fixture/fixture', prompt]
        if args.trace:
            command.extend(['--print-logs', '--log-level', 'DEBUG'])
    elif args.agent == 'zcode':
        config_dir = fixture / 'home/.zcode/agent'
        config_dir.mkdir(parents=True)
        model_config = dict(providers=dict(fixture=dict(baseUrl=base_url, api='openai-completions',
            apiKey='local-fixture-only', models=[dict(id='fixture', name='Fixture', reasoning=False,
                input=['text'], contextWindow=200000, maxTokens=4096,
                cost=dict(input=0, output=0, cacheRead=0, cacheWrite=0))])))
        (config_dir / 'models.yml').write_text(json.dumps(model_config, indent=2), encoding='utf-8')
        env.extend([f'PI_CODING_AGENT_DIR={guest}/home/.zcode/agent', 'PI_EDIT_VARIANT=replace'])
        if args.trace:
            env.append('PI_DEBUG_STARTUP=1')
        command = [args.cli, '--provider', 'fixture', '--model', 'fixture', '--print', '--mode', 'json',
                   '--no-session', '--no-extensions', '--no-lsp', '--no-skills', '--no-rules', '--no-title',
                   '--approval-mode', 'yolo',
                   '--tools', 'read,write,edit,bash,grep', prompt]
    else:
        env.extend([f'DSH_HOME={guest}/home/.dsh', f'DEEPSEEK_BASE_URL={base_url}',
                    'DEEPSEEK_API_KEY=local-fixture-only', 'DSH_PERMISSION_MODE=danger-full-access',
                    'DSH_TELEMETRY_MODE=DISABLED'])
        command = [args.node, args.cli, '--profile', 'headless', '--json', prompt]
    report['command'] = command
    persist()
    thread.start()
    watcher = None
    try:
        with InitPool(root, dist, output / 'baseline-session', timeout=30, size=1) as pool:
            report['phase'] = 'baseline'
            report['baseline'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest,
                expected_exit=1, expect=['FAILED (failures=2)'])
            persist()
        with InitPool(root, dist, output / 'session', timeout=args.timeout, size=1,
                      memory_limit_bytes=args.memory_limit_mb * 1024**2) as pool:
            active_pool = pool
            watcher = AgentResourceWatch(pool, max_private_commit=args.memory_limit_mb * 1024**2)
            report['phase'] = 'agent'
            persist()
            report['agent_run'] = pool.run(['/usr/bin/env', '-i', *env, *command], cwd=guest,
                expect=['EXTRA_AGENT_WORKFLOW_DONE'])
            persist()
        report['resources'] = watcher.finish()
        active_pool = None
        watcher = None
        with InitPool(root, dist, output / 'verification-session', timeout=30, size=1) as pool:
            report['phase'] = 'verification'
            report['verification'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest, expect=['Ran 3 tests', 'OK'])
        report['edit_correct'] = (fixture / 'daily.py').read_bytes() == SOURCE.replace('values[:-1]', 'values').encode()
        report['tests_preserved'] = (fixture / 'test_daily.py').read_bytes() == TESTS.encode()
        report['unicode_correct'] = (fixture / note).is_file() and (fixture / note).read_bytes() == '已修改 ✅\n'.encode()
        report['passed'] = (done.is_set() and len(results) == len(actions) and not errors
            and all(row['passed'] for row in results)
            and report['resources']['cleanup_passed'] and not report['resources']['limit_exceeded']
            and all(report[key] for key in ('edit_correct', 'tests_preserved', 'unicode_correct'))
            and all(report[key]['status'] == 'passed' for key in ('baseline', 'agent_run', 'verification')))
    except Exception as error:
        report['error'] = str(error)
        report['timed_out'] = pool.timed_out.is_set() if 'pool' in locals() else False
    finally:
        if watcher:
            report['resources'] = watcher.finish()
            report['passed'] = report['passed'] and report['resources']['cleanup_passed']
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        report['requests'] = len(requests)
        persist()
    print(json.dumps(dict(agent=args.agent, passed=report['passed'], completed=len(results), planned=len(actions),
                         errors=errors, error=report.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
