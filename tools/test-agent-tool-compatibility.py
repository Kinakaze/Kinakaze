"""Exercise real agent tools against a deterministic local model provider.

Includes search, Unicode paths, edits, large output and subprocess cancellation.
No online model, account credentials or third-party services are used.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import shlex
import threading
import uuid

from init_pool import InitPool, distribution_hashes


SOURCE = '# 求和 🧪\ndef total(values):\n    return sum(values[:-1])\n'
TESTS = ('import unittest\nfrom daily import total\nclass Tests(unittest.TestCase):\n'
         '    def test_empty(self): self.assertEqual(total([]), 0)\n'
         '    def test_one(self): self.assertEqual(total([7]), 7)\n'
         '    def test_many(self): self.assertEqual(total([2, 3, 4]), 9)\n')


def tool_output(request, call_id, codex):
    if codex:
        blocks = [item for item in request.get('input', []) if isinstance(item, dict)
                  and item.get('type') in ('function_call_output', 'custom_tool_call_output', 'tool_search_output')
                  and item.get('call_id') == call_id]
        if not blocks:
            raise ValueError('missing tool result for ' + call_id)
        return blocks[-1].get('output', blocks[-1].get('tools', '')), False
    blocks = [block for message in request.get('messages', [])
              if isinstance(message.get('content'), list) for block in message['content']
              if isinstance(block, dict) and block.get('type') == 'tool_result'
              and block.get('tool_use_id') == call_id]
    if not blocks:
        raise ValueError('missing tool result for ' + call_id)
    return blocks[-1].get('content', ''), bool(blocks[-1].get('is_error'))


class Workflow:
    def __init__(self, agent, guest, fixture, mcp=False):
        self.agent, self.guest, self.fixture = agent, guest, fixture
        self.actions, self.results, self.requests, self.errors = [], [], [], []
        self.lock = threading.Lock()
        self.session_id = None
        self.spill_path = None
        self.current_namespace = None
        self.background_id = None
        self.background_path = None
        self.note = '目录 space/子目录/note.txt'
        self.add('read-source', 'read', {'path': 'daily.py'}, ['求和', 'sum(values[:-1])'])
        if mcp:
            if agent == 'codex':
                self.add('mcp-tool-discovery', 'discover', {'query': 'fixture echo inspect', 'limit': 2}, ['echo', 'inspect'])
            self.add('mcp-unicode-echo', 'mcp', {'name': 'echo', 'arguments': {'text': '中文 space 🧪'}}, ['MCP_ECHO:中文 space 🧪'])
            self.add('mcp-inspect-file', 'mcp', {'name': 'inspect', 'arguments': {}}, [hashlib.sha256(SOURCE.encode()).hexdigest()])
        if agent != 'codex':
            self.add('list-files', 'ls', {'path': '.'}, ['daily.py', 'test_daily.py'])
            self.add('find-files', 'find', {'pattern': '**/*.py', 'path': '.'}, ['daily.py', 'test_daily.py'])
            self.add('search-source', 'grep', {'pattern': 'values', 'path': '.', 'glob': '*.py'}, ['daily.py', 'values'])
        self.shell('failing-tests', 'python3 -m unittest -v', ['FAILED (failures=2)'], error=agent != 'codex')
        self.add('write-unicode-path', 'write', {'path': self.note, 'content': '你好 🧪\n'}, [])
        self.add('read-unicode-path', 'read', {'path': self.note}, ['你好 🧪'])
        self.add('rejected-edit', 'edit', {'path': 'daily.py', 'old': 'DOES_NOT_EXIST', 'new': 'invalid'},
                 ['DOES_NOT_EXIST'] if agent != 'pi' else ['Could not find'], error=True)
        self.add('edit-source', 'edit', {'path': 'daily.py', 'old': 'sum(values[:-1])', 'new': 'sum(values)'}, [])
        self.add('edit-unicode-path', 'edit', {'path': self.note, 'old': '你好 🧪', 'new': '已修改 ✅'}, [])
        self.add('read-edited-path', 'read', {'path': self.note}, ['已修改 ✅'])
        self.shell('passing-tests', 'python3 -m unittest -v', ['Ran 3 tests', 'OK'])
        code = '[print("line-%04d" % n) for n in range(3500)]; print("LARGE_OUTPUT_END")'
        markers = ['line-0000', 'Full output saved to:'] if agent == 'claude' else ['LARGE_OUTPUT_END']
        self.shell('large-output', 'python3 -c ' + shlex.quote(code), markers)
        if agent == 'claude':
            self.add('read-large-output', 'spill', {}, ['LARGE_OUTPUT_END'])
        self.shell('stderr-output', "python3 -c 'import sys; print(\"STDERR_ONLY_OK\", file=sys.stderr)'", ['STDERR_ONLY_OK'])
        if agent == 'codex':
            self.add('move-patch', 'patch', '*** Begin Patch\n*** Update File: ' + self.note +
                     '\n*** Move to: 目录 space/moved.txt\n@@\n-已修改 ✅\n+移动成功 ✅\n*** End Patch\n', [])
            self.shell('verify-move', "test ! -e '目录 space/子目录/note.txt' && cat '目录 space/moved.txt'", ['移动成功 ✅'])
            self.shell('interactive-start', "python3 -u -c 'print(\"INPUT_READY\"); print(\"INPUT_RECEIVED:\" + input())'",
                       ['INPUT_READY'], tty=True, yield_time_ms=1000)
            self.add('interactive-input', 'stdin', {'chars': '中文 input\n', 'yield_time_ms': 1000}, ['INPUT_RECEIVED:中文 input'])
            self.shell('interrupt-start', "trap 'printf INTERRUPTED_OK; exit 130' INT; printf 'INTERRUPT_READY\\n'; sleep 60",
                       ['INTERRUPT_READY'], tty=True, yield_time_ms=1000)
            self.add('interrupt-input', 'stdin', {'chars': '\x03', 'yield_time_ms': 1000}, ['INTERRUPTED_OK', 'Process exited with code 130'])
        else:
            code = 'import os,time; from pathlib import Path; Path("background.pid").write_text(str(os.getpid())); print("TIMEOUT_READY", flush=True); time.sleep(60); Path("timeout-leaked").write_text("bad")'
            if agent == 'claude':
                self.shell('background-timeout', 'python3 -u -c ' + shlex.quote(code), ['moved to the background', 'ID:'], timeout=1)
                self.add('background-output', 'task-output', {}, ['TIMEOUT_READY'])
                self.add('background-stop', 'task-stop', {}, [])
            else:
                self.shell('shell-timeout', 'python3 -u -c ' + shlex.quote(code), ['TIMEOUT_READY'], error=True, timeout=1)
        self.shell('after-cancellation', 'python3 -m unittest -v; test ! -e timeout-leaked; echo AFTER_CANCELLATION_OK',
                   ['Ran 3 tests', 'OK', 'AFTER_CANCELLATION_OK'])

    def add(self, case, kind, arguments, contains, error=False, **options):
        self.actions.append(dict(case=case, kind=kind, arguments=arguments, contains=contains, error=error, **options))

    def shell(self, case, command, contains, error=False, **options):
        self.add(case, 'shell', command, contains, error, **options)

    def validate(self, request, index):
        action = self.actions[index]
        output, is_error = tool_output(request, f'fixture-{index}', self.agent == 'codex')
        text = output if isinstance(output, str) else json.dumps(output, ensure_ascii=False)
        missing = [marker for marker in action['contains'] if marker not in text]
        valid = not missing and (self.agent == 'codex' or is_error == action['error'])
        if action['case'] == 'rejected-edit':
            valid = valid and (self.fixture / 'daily.py').read_bytes() == SOURCE.encode()
        if action['case'] == 'background-stop':
            valid = valid and self.background_id in text and 'stop' in text.lower()
        self.results.append(dict(case=action['case'], tool=action.get('tool'), passed=valid, missing=missing, is_error=is_error, output=output))
        if not valid:
            raise ValueError(f"{action['case']}: missing={missing}, is_error={is_error}, expected_error={action['error']}")
        if action['case'] == 'large-output' and self.agent == 'claude':
            match = re.search(r'Full output saved to:\s*([^\n]+)', text)
            if not match or not match[1].startswith(self.guest + '/'):
                raise ValueError('missing fixture output file')
            self.spill_path = match[1].strip()
        if action['case'] == 'background-timeout':
            match = re.search(r'\(ID:\s*([\w-]+)\)', text)
            if not match:
                raise ValueError('missing background task ID')
            self.background_id = match[1]
            match = re.search(r'Output is being written to:\s*(/[^\s]+)', text)
            if not match:
                raise ValueError('missing background output file')
            self.background_path = match[1].rstrip('.')
        if action['case'] in ('interactive-start', 'interrupt-start'):
            match = re.search(r'session ID\s+(\d+)', text, re.IGNORECASE)
            if not match:
                raise ValueError('missing running session ID: ' + text)
            self.session_id = int(match[1])

    def call(self, action, request):
        kind, args = action['kind'], action['arguments']
        self.current_namespace = None
        if kind == 'task-output':
            return 'Read', dict(file_path=self.background_path), False
        if kind == 'task-stop':
            if 'TaskStop' in {tool.get('name') for tool in request.get('tools', [])}:
                return 'TaskStop', dict(task_id=self.background_id), False
            code = ('import os,signal,time; from pathlib import Path; pid=int(Path("background.pid").read_text()); '
                    'os.kill(pid,signal.SIGTERM)\n'
                    'for _ in range(100):\n'
                    ' try: os.kill(pid,0)\n'
                    ' except ProcessLookupError: break\n'
                    ' time.sleep(.05)\n'
                    'else: raise RuntimeError("background process still alive")\n'
                    'print(' + repr('Stopped background task ' + self.background_id) + ')')
            return 'Bash', dict(command='python3 -c ' + shlex.quote(code), description='Stop fixture background process'), False
        if kind == 'spill':
            return 'Bash', dict(command='tail -n 3 -- ' + shlex.quote(self.spill_path), description='Read full persisted output'), False
        if kind == 'discover':
            if not any(tool.get('type') == 'tool_search' for tool in request.get('tools', [])):
                raise ValueError('tool search not advertised')
            return 'tool_search', args, False
        if kind == 'mcp':
            name = 'mcp__fixture__' + args['name']
            advertised = list(request.get('tools', []))
            for item in request.get('input', []):
                if isinstance(item, dict) and item.get('type') == 'tool_search_output':
                    advertised.extend(item.get('tools', []))
            if name in {tool.get('name') for tool in advertised}:
                return name, args['arguments'], False
            for tool in advertised:
                if tool.get('type') == 'namespace' and tool.get('name') == 'mcp__fixture':
                    if args['name'] in {child.get('name') for child in tool.get('tools', [])}:
                        self.current_namespace = tool['name']
                        return args['name'], args['arguments'], False
            raise ValueError('MCP tool not advertised: ' + name)
        if self.agent == 'codex':
            tools = {tool.get('name'): tool for tool in request.get('tools', [])}
            if kind in ('edit', 'write', 'patch'):
                if kind == 'edit':
                    patch = ('*** Begin Patch\n*** Update File: ' + args['path'] + '\n@@\n-' + args['old'] +
                             '\n+' + args['new'] + '\n*** End Patch\n')
                    if args['path'] == 'daily.py' and action['case'] == 'edit-source':
                        patch = patch.replace('-sum(', '-    return sum(').replace('+sum(', '+    return sum(')
                elif kind == 'write':
                    patch = ('*** Begin Patch\n*** Add File: ' + args['path'] + '\n' +
                             ''.join('+' + line + '\n' for line in args['content'].splitlines()) + '*** End Patch\n')
                else:
                    patch = args
                tool = tools.get('apply_patch')
                if tool is None:
                    raise ValueError('apply_patch not advertised')
                return 'apply_patch', patch, tool['type'] == 'custom'
            if kind == 'stdin':
                return 'write_stdin', dict(session_id=self.session_id, **args), False
            command = ('cat -- ' + shlex.quote(args['path'])) if kind == 'read' else args
            return 'exec_command', dict(cmd=command, workdir=self.guest, max_output_tokens=2000,
                                       **{key: action[key] for key in ('tty', 'yield_time_ms') if key in action}), False
        claude = self.agent == 'claude'
        names = {'read': 'Read', 'write': 'Write', 'edit': 'Edit', 'ls': 'Bash', 'find': 'Glob', 'grep': 'Grep', 'shell': 'Bash'}
        name = names[kind] if claude else ('bash' if kind == 'shell' else kind)
        if kind in ('read', 'write', 'edit'):
            arguments = {('file_path' if claude else 'path'): self.guest + '/' + args['path']}
            if kind == 'write':
                arguments['content'] = args['content']
            if kind == 'edit':
                if claude:
                    arguments.update(old_string=args['old'], new_string=args['new'])
                else:
                    schema = next(tool['input_schema'] for tool in request['tools'] if tool['name'] == name)
                    if 'edits' in schema.get('properties', {}):
                        arguments['edits'] = [dict(oldText=args['old'], newText=args['new'])]
                    else:
                        arguments.update(oldText=args['old'], newText=args['new'])
        elif kind == 'ls' and claude:
            arguments = dict(command='ls -a', description=action['case'])
        elif kind in ('ls', 'find', 'grep'):
            arguments = {**args, 'path': self.guest}
            if kind == 'grep' and claude:
                arguments['output_mode'] = 'content'
        else:
            arguments = dict(command=args)
            if claude:
                arguments['description'] = action['case']
            if 'timeout' in action:
                arguments['timeout'] = action['timeout'] * (1000 if claude else 1)
        if name not in {tool.get('name') for tool in request.get('tools', [])}:
            raise ValueError('required tool not advertised: ' + name)
        return name, arguments, False

    def respond(self, request):
        with self.lock:
            index = len(self.requests)
            self.requests.append(request)
            if index > len(self.actions):
                raise ValueError('unexpected extra request')
            if index:
                self.validate(request, index - 1)
            if index < len(self.actions):
                name, args, custom = self.call(self.actions[index], request)
                self.actions[index]['tool'] = (self.current_namespace + '.' if self.current_namespace else '') + name
            else:
                name, args, custom = None, None, False
            if self.agent == 'codex':
                if name == 'tool_search':
                    item = dict(id=f'item-{index}', type='tool_search_call', execution='client',
                                call_id=f'fixture-{index}', status='completed', arguments=args)
                elif name:
                    item = dict(id=f'item-{index}', type='custom_tool_call' if custom else 'function_call',
                                call_id=f'fixture-{index}', name=name, status='completed')
                    if self.current_namespace:
                        item['namespace'] = self.current_namespace
                    item['input' if custom else 'arguments'] = args if custom else json.dumps(args, ensure_ascii=False)
                else:
                    item = dict(id=f'item-{index}', type='message', role='assistant', status='completed',
                                content=[dict(type='output_text', text='AGENT_TOOLS_DONE', annotations=[])])
                response = dict(id=f'response-{index}', object='response', created_at=0, status='completed',
                                output=[item], model=request.get('model'), usage=dict(input_tokens=1, output_tokens=1, total_tokens=2))
                events = [dict(type='response.created', response={**response, 'status': 'in_progress', 'output': []}),
                          dict(type='response.output_item.added', output_index=0, item=item),
                          dict(type='response.output_item.done', output_index=0, item=item),
                          dict(type='response.completed', response=response)]
                return ''.join(f'event: {event["type"]}\ndata: {json.dumps(event, ensure_ascii=False)}\n\n' for event in events).encode()
            block = dict(type='tool_use', id=f'fixture-{index}', name=name, input={}) if name else dict(type='text', text='')
            delta = dict(type='input_json_delta', partial_json=json.dumps(args, ensure_ascii=False)) if name else dict(type='text_delta', text='AGENT_TOOLS_DONE')
            events = [dict(type='message_start', message=dict(id=f'message-{index}', type='message', role='assistant',
                           model=request.get('model'), content=[], stop_reason=None, stop_sequence=None, usage=dict(input_tokens=1, output_tokens=0))),
                      dict(type='content_block_start', index=0, content_block=block),
                      dict(type='content_block_delta', index=0, delta=delta), dict(type='content_block_stop', index=0),
                      dict(type='message_delta', delta=dict(stop_reason='tool_use' if name else 'end_turn', stop_sequence=None), usage=dict(output_tokens=1)),
                      dict(type='message_stop')]
            return ''.join(f'event: {event["type"]}\ndata: {json.dumps(event, ensure_ascii=False)}\n\n' for event in events).encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('root', 'dist', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    parser.add_argument('--agent', choices=('codex', 'claude', 'pi'), required=True)
    parser.add_argument('--cli', required=True, help='absolute guest CLI binary or pi script')
    parser.add_argument('--node', default='/root/node-v22.23.3-linux-x64/bin/node')
    parser.add_argument('--tool-bin', help='absolute guest directory containing search tools')
    parser.add_argument('--mcp', action='store_true', help='also exercise a local Python JSON-RPC stdio server (Codex/Claude)')
    parser.add_argument('--timeout', type=float, default=90, help='owned session timeout in seconds')
    parser.add_argument('--memory-limit-mb', type=int, default=16384,
                        help='committed-memory limit for this owned process tree')
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    if args.memory_limit_mb <= 0:
        parser.error('--memory-limit-mb must be positive')
    if args.mcp and args.agent == 'pi':
        parser.error('pi MCP extensions are outside this built-in-tool test')
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    for path in [args.cli, *([args.node] if args.agent == 'pi' else [])]:
        if not path.startswith('/') or not (root / path.lstrip('/')).is_file():
            parser.error('missing absolute guest executable: ' + path)
    guest = '/tmp/agent-tools-' + uuid.uuid4().hex
    fixture = root / guest.lstrip('/')
    (fixture / 'home').mkdir(parents=True)
    for name, content in [('daily.py', SOURCE), ('test_daily.py', TESTS)]:
        (fixture / name).write_text(content, encoding='utf-8', newline='\n')
    workflow = Workflow(args.agent, guest, fixture, args.mcp)
    active_pool = None
    if args.mcp:
        source = Path(__file__).resolve().parents[1] / 'tests/guest/AgentMcpFixture.py'
        (fixture / 'mcp_fixture.py').write_bytes(source.read_bytes())

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 < length < 8 * 1024 * 1024:
                    self.send_error(400)
                    return
                request = json.loads(self.rfile.read(length))
                if self.path.startswith('/v1/messages/count_tokens'):
                    body, content_type = b'{"input_tokens":100}', 'application/json'
                elif self.path.split('?')[0] == ('/v1/responses' if args.agent == 'codex' else '/v1/messages'):
                    body, content_type = workflow.respond(request), 'text/event-stream'
                    (output / 'progress.json').write_text(json.dumps(dict(guest=guest, requests=len(workflow.requests),
                        completed=len(workflow.results), results=workflow.results, errors=workflow.errors),
                        indent=2, ensure_ascii=False), encoding='utf-8')
                else:
                    raise ValueError('unexpected endpoint: ' + self.path)
                self.send_response(200)
                self.send_header('Content-Type', content_type)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except Exception as error:
                workflow.errors.append(str(error))
                self.send_error(400)
                if active_pool is not None:
                    # End only this owned test session; failed model turns
                    # otherwise leave background tasks running until timeout.
                    active_pool.child.close()

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    if args.tool_bin and not args.tool_bin.startswith('/'):
        parser.error('--tool-bin must be an absolute guest directory')
    search_path = (args.tool_bin + ':' if args.tool_bin else '') + '/usr/bin:/bin'
    environment = [f'HOME={guest}/home', 'PATH=' + search_path, 'LANG=C.UTF-8', 'TERM=dumb']
    if args.agent == 'codex':
        (fixture / 'home/.codex').mkdir()
        command = ['/usr/bin/env', '-i', *environment, f'CODEX_HOME={guest}/home/.codex', 'OPENAI_API_KEY=local-fixture-only',
                   args.cli, 'exec', '--skip-git-repo-check', '--ephemeral', '--dangerously-bypass-approvals-and-sandbox', '--json', '--model', 'gpt-5.4']
        for setting in ['model_provider="fixture"', 'model_providers.fixture.name="Fixture"',
                        f'model_providers.fixture.base_url="http://127.0.0.1:{server.server_port}/v1"',
                        'model_providers.fixture.wire_api="responses"', 'model_providers.fixture.env_key="OPENAI_API_KEY"',
                        'model_providers.fixture.supports_websockets=false', 'model_providers.fixture.request_max_retries=0']:
            command.extend(['-c', setting])
        if args.mcp:
            for setting in ['mcp_servers.fixture.command="/usr/bin/python3"',
                            'mcp_servers.fixture.args=' + json.dumps([guest + '/mcp_fixture.py'])]:
                command.extend(['-c', setting])
    elif args.agent == 'claude':
        mcp_config = dict(mcpServers=dict(fixture=dict(type='stdio', command='/usr/bin/python3',
                          args=[guest + '/mcp_fixture.py']))) if args.mcp else dict(mcpServers={})
        builtins = 'Read,Edit,Write,Glob,Grep,Bash,TaskStop'
        allowed = builtins + (',mcp__fixture__echo,mcp__fixture__inspect' if args.mcp else '')
        command = ['/usr/bin/env', '-i', *environment, f'CLAUDE_CONFIG_DIR={guest}/home/.claude',
                   f'ANTHROPIC_BASE_URL=http://127.0.0.1:{server.server_port}', 'ANTHROPIC_API_KEY=local-fixture-only',
                   'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1', 'DISABLE_AUTOUPDATER=1', args.cli,
                   '--disable-slash-commands', '--strict-mcp-config', '--mcp-config', json.dumps(mcp_config),
                   '--tools', builtins,
                   '--no-session-persistence', '--allowedTools', allowed, '--model', 'claude-sonnet-4-6',
                   '--debug-file', guest + '/home/claude.debug.log',
                   '--output-format', 'stream-json', '--verbose', '-p']
    else:
        config = fixture / 'home/.pi/agent'
        config.mkdir(parents=True)
        (config / 'models.json').write_text(json.dumps(dict(providers=dict(compatibility=dict(
            baseUrl=f'http://127.0.0.1:{server.server_port}', api='anthropic-messages', apiKey='local-fixture-only',
            models=[dict(id='pi-fixture', contextWindow=200000, maxTokens=4096)])))), encoding='utf-8')
        command = ['/usr/bin/env', '-i', *environment, f'PI_CODING_AGENT_DIR={guest}/home/.pi/agent', args.node, args.cli,
                   '--offline', '--no-session', '--no-extensions', '--no-skills', '--no-context-files',
                   '--tools', 'read,bash,edit,write,grep,find,ls', '--provider', 'compatibility', '--model', 'pi-fixture', '--print']
    command.append('Exercise the tools on this fixture, fix daily.py, and verify the tests. Preserve test_daily.py.')
    report = dict(scope=__doc__, agent=args.agent, root=str(root), dist=str(dist), guest=guest,
                  sha256=distribution_hashes(dist), harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  cli_sha256=hashlib.sha256((root / args.cli.lstrip('/')).read_bytes()).hexdigest(), passed=False)
    prerequisites = [args.node] if args.agent == 'pi' else []
    if args.tool_bin:
        prerequisites.append(args.tool_bin.rstrip('/') + '/fd')
    prerequisites.append('/usr/bin/rg')
    report['prerequisites_sha256'] = {path: hashlib.sha256((root / path.lstrip('/')).read_bytes()).hexdigest()
                                    for path in prerequisites if (root / path.lstrip('/')).is_file()}
    thread.start()
    try:
        with InitPool(root, dist, output / 'session', size=1, timeout=args.timeout,
                      memory_limit_bytes=args.memory_limit_mb * 1024**2) as pool:
            active_pool = pool
            report['agent_run'] = pool.run(command, cwd=guest, expect=['AGENT_TOOLS_DONE'])
            report['verification'] = pool.run(['/usr/bin/python3', '-m', 'unittest', '-v'], cwd=guest, expect=['Ran 3 tests', 'OK'])
            report['memory_metrics'] = pool.child.memory_metrics()
        report['edit_correct'] = (fixture / 'daily.py').read_bytes() == SOURCE.replace('values[:-1]', 'values').encode()
        report['tests_preserved'] = (fixture / 'test_daily.py').read_bytes() == TESTS.encode()
        note = '目录 space/moved.txt' if args.agent == 'codex' else workflow.note
        expected = '移动成功 ✅\n' if args.agent == 'codex' else '已修改 ✅\n'
        report['unicode_file_correct'] = (fixture / note).is_file() and (fixture / note).read_bytes() == expected.encode()
        report['cancellation_preserved'] = not (fixture / 'timeout-leaked').exists()
        report['mcp_validated'] = True
        if args.mcp:
            log = fixture / 'mcp-invocations.jsonl'
            calls = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()] if log.is_file() else []
            report['mcp_validated'] = ([call['name'] for call in calls] == ['echo', 'inspect']
                and calls[0]['text'] == 'MCP_ECHO:中文 space 🧪'
                and json.loads(calls[1]['text']) == dict(source=SOURCE, sha256=hashlib.sha256(SOURCE.encode()).hexdigest()))
        report['passed'] = (len(workflow.results) == len(workflow.actions) and not workflow.errors
                            and all(row['passed'] for row in workflow.results)
                            and all(report[name] for name in ('edit_correct', 'tests_preserved', 'unicode_file_correct', 'cancellation_preserved', 'mcp_validated'))
                            and all(report[name]['status'] == 'passed' for name in ('agent_run', 'verification')))
    except Exception as error:
        report['error'] = str(error)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        report.update(results=workflow.results, requests=len(workflow.requests), provider_errors=workflow.errors)
        (output / 'requests.json').write_text(json.dumps(workflow.requests, indent=2, ensure_ascii=False), encoding='utf-8')
        (output / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')
    print(json.dumps(dict(agent=args.agent, passed=report['passed'], completed=len(workflow.results),
                         planned=len(workflow.actions), errors=workflow.errors, error=report.get('error'))))
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
