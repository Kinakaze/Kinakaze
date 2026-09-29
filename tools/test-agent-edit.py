"""Exercise a model-driven edit/test/review loop using real Kinakaze guest tools.

Credentials come from AGENT_API_KEY, never command-line flags or report files.
Only the generated fixture and tool results are sent to the configured endpoint.
"""
import argparse
import json
import os
from pathlib import Path
import runpy
import shlex
import shutil
import sys
import urllib.request
import uuid
from init_pool import distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--base-url', default=os.environ.get('AGENT_BASE_URL', ''))
    parser.add_argument('--model', default=os.environ.get('AGENT_MODEL', ''))
    parser.add_argument('--offline', action='store_true', help='run guest proc/editor regression only')
    parser.add_argument('--claude', help='guest Claude executable for the Bash Ctrl-C probe')
    parser.add_argument('--text-tools', action='store_true', help='use one JSON command per model turn for services without native tool calls')
    args = parser.parse_args()
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    guest = '/tmp/agent-edit-' + uuid.uuid4().hex
    stage = root / guest.lstrip('/')
    stage.mkdir(parents=True)
    (stage / 'daily.py').write_text('def total(values):\n    return sum(values[:-1])\n', encoding='utf-8')
    (stage / 'test_daily.py').write_text('import unittest\nfrom daily import total\n'
        'class Tests(unittest.TestCase):\n'
        '    def test_all(self): self.assertEqual(total([2, 3, 5]), 10)\n'
        '    def test_empty(self): self.assertEqual(total([]), 0)\n'
        '    def test_single(self): self.assertEqual(total([7]), 7)\n'
        '    def test_negative(self): self.assertEqual(total([-2, 3]), 1)\n', encoding='utf-8')
    shutil.copyfile(stage / 'daily.py', stage / 'daily.py.before')
    original_tests = (stage / 'test_daily.py').read_bytes()
    original_source = (stage / 'daily.py.before').read_bytes()
    shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/ProcInspectionProbe.py', stage / 'proc_probe.py')
    shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/BashInterruptProbe.py', stage / 'bash_probe.py')
    shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/FilesystemBoundaryProbe.py', stage / 'filesystem_probe.py')
    execute = runpy.run_path(str(Path(__file__).with_name('test-debian-commands.py')))['execute']
    env = {k: v for k, v in os.environ.items() if not k.startswith(
        ('AGENT_', 'OPENAI_', 'ANTHROPIC_', 'CODEX_', 'CLAUDE_', 'KINAKAZE_'))}
    env.update(TERM='dumb', LC_ALL='C.UTF-8')
    report = dict(guest=guest, model=args.model, sha256=distribution_hashes(dist), steps=[], passed=False)
    def save():
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    def run(command, extra_env=None):
        row = execute([str(dist / 'worker.exe'), 'oneshot', '--root', str(root), '--dist', str(dist),
                       '--cwd', guest, '--', '/bin/bash', '-c',
                       'export PATH=/usr/bin:/bin; ' + command],
                      output / f'step-{len(report["steps"])}',
                      {**env, **{'KINAKAZE_GUEST_ENV_' + k: v for k, v in (extra_env or {}).items()}}, timeout=90)
        report['steps'].append(dict(command=command, **row))
        save()
        print(json.dumps(dict(step=len(report['steps']), exit_code=row['exit_code'],
                              timed_out=row['timed_out'])), flush=True)
        return json.dumps({k: row[k] for k in ('exit_code', 'timed_out', 'stdout', 'stderr')})
    run('python3 proc_probe.py')
    report['proc_passed'] = report['steps'][-1]['exit_code'] == 0 and 'PROC_INSPECTION_OK' in report['steps'][-1]['stdout']
    run('python3 filesystem_probe.py')
    report['filesystem_boundaries_passed'] = report['steps'][-1]['exit_code'] == 0 and 'FILESYSTEM_BOUNDARIES_OK' in report['steps'][-1]['stdout']
    cli_env = {}
    if args.claude and args.base_url and os.environ.get('AGENT_API_KEY'):
        cli_env = dict(ANTHROPIC_BASE_URL=args.base_url, ANTHROPIC_API_KEY=os.environ['AGENT_API_KEY'],
                       CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1', DISABLE_AUTOUPDATER='1')
    run('python3 bash_probe.py' + (' ' + shlex.quote(args.claude) if args.claude else ''), cli_env)
    report['bash_interrupt_passed'] = report['steps'][-1]['exit_code'] == 0 and 'BASH_INTERRUPT_OK' in report['steps'][-1]['stdout']
    run('python3 -m unittest -v')
    report['baseline_failed'] = report['steps'][-1]['exit_code'] != 0 and 'FAILED (failures=3)' in report['steps'][-1]['stderr']
    if args.offline:
        run("python3 -c \"from pathlib import Path; p=Path('daily.py'); p.write_text(p.read_text().replace('values[:-1]', 'values'))\"")
    else:
        key = os.environ['AGENT_API_KEY']
        base = args.base_url.rstrip('/')
        if not base.endswith('/v1'):
            base += '/v1'
        def request(path, payload=None):
            req = urllib.request.Request(base + path, headers={
                'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'},
                data=None if payload is None else json.dumps(payload).encode())
            with urllib.request.urlopen(req, timeout=90) as response:
                return json.load(response)
        if not args.model:
            models = [item['id'] for item in request('/models')['data']]
            # Prefer a general tool-capable model if advertised; record exact ID.
            args.model = next((m for m in ('gpt-5.4', 'gpt-5.2', 'gpt-4.1', 'gpt-4o') if m in models), models[0])
        report['model'] = args.model
        messages = [dict(role='system', content='You are testing a Linux development sandbox. '
            'Use shell to inspect files, fix daily.py so total sums every input including empty and negative inputs, '
            'run existing unittest tests, and inspect diff -u daily.py.before daily.py. '
            'Work only in the current fixture directory. Do not modify tests or the before file. '
            'Use python3 and standard shell tools. Finish after verification.'),
            dict(role='user', content='Please fix the total function and verify the edit with tests and a diff.')]
        if args.text_tools:
            messages[0]['content'] += (' Protocol: reply ONLY with one JSON object {"command":"shell command"}. '
                'The caller will execute it and return actual stdout/stderr. Never invent tool results. '
                'When verified, reply {"done":true}. No markdown or explanations.')
        for _ in range(12):
            payload = dict(model=args.model, messages=messages, max_tokens=2000)
            if not args.text_tools:
                payload['tools'] = [dict(type='function', function=dict(name='shell', description='Run a shell command in the Linux guest fixture.',
                    parameters=dict(type='object', properties=dict(command=dict(type='string')), required=['command'])))]
            response = request('/chat/completions', payload)
            message = response['choices'][0]['message']
            messages.append(message)
            if args.text_tools:
                content = (message.get('content') or '').strip()
                if content.startswith('```'):
                    content = '\n'.join(content.splitlines()[1:-1])
                try:
                    action = json.loads(content)
                except json.JSONDecodeError:
                    messages.append(dict(role='user', content='Invalid JSON: send one valid JSON object. Escape backslashes and quotes correctly. No command was executed.'))
                    continue
                if action.get('done'):
                    report['final_message'] = 'done'
                    break
                messages.append(dict(role='user', content=run(action['command'])))
                continue
            calls = message.get('tool_calls', [])
            if not calls:
                report['final_message'] = message.get('content')
                break
            for call in calls:
                if call['function']['name'] != 'shell':
                    raise ValueError('unknown tool')
                result = run(json.loads(call['function']['arguments'])['command'])
                messages.append(dict(role='tool', tool_call_id=call['id'], content=result))
    run('python3 -m unittest -v')
    final = report['steps'][-1]
    report['tests_passed'] = final['exit_code'] == 0 and 'Ran 4 tests' in final['stderr'] and 'OK' in final['stderr']
    run('diff -u daily.py.before daily.py')
    report['diff_present'] = report['steps'][-1]['exit_code'] == 1 and '+    return ' in report['steps'][-1]['stdout']
    report['fixtures_preserved'] = ((stage / 'test_daily.py').read_bytes() == original_tests
                                    and (stage / 'daily.py.before').read_bytes() == original_source)
    report['agent_edit_passed'] = all(report.get(k) for k in ('baseline_failed', 'tests_passed', 'diff_present', 'fixtures_preserved'))
    report['passed'] = all(report.get(k) for k in ('proc_passed', 'filesystem_boundaries_passed', 'bash_interrupt_passed', 'agent_edit_passed'))
    save()
    print(json.dumps({k: v for k, v in report.items() if k not in ('steps', 'sha256')}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
