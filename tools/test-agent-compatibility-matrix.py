"""Repeat real agent tool workflows, preserving each failure and its logs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

from init_pool import distribution_hashes
from session_process import SessionProcess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('root', 'dist', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    parser.add_argument('--agents', nargs='+', choices=('codex', 'claude', 'pi'), default=['codex', 'claude', 'pi'])
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--timeout', type=float, default=90, help='timeout of each owned session in seconds')
    parser.add_argument('--memory-limit-mb', type=int, default=16384,
                        help='committed-memory limit for each owned process tree')
    parser.add_argument('--pi-root', type=Path, help='separate root containing pi and Node')
    parser.add_argument('--node', default='/root/node-v22.23.3-linux-x64/bin/node')
    parser.add_argument('--tool-bin', help='pi search tool directory in the guest')
    parser.add_argument('--mcp', action='store_true', help='include local MCP tools for Codex and Claude')
    for agent in ('codex', 'claude', 'pi'):
        parser.add_argument('--' + agent, help='absolute guest CLI executable/script')
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('--repeat must be positive')
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    if args.memory_limit_mb <= 0:
        parser.error('--memory-limit-mb must be positive')
    if len(set(args.agents)) != len(args.agents):
        parser.error('--agents must not contain duplicates')
    for agent in args.agents:
        if not getattr(args, agent):
            parser.error('--' + agent + ' is required for the selected agent')
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    if output.exists():
        parser.error('--output must be new to preserve previous reports')
    output.mkdir(parents=True)
    harness = Path(__file__).with_name('test-agent-tool-compatibility.py')
    report = dict(scope=__doc__, root=str(root), dist=str(dist), sha256=distribution_hashes(dist),
                  harness_sha256=hashlib.sha256(harness.read_bytes()).hexdigest(),
                  planned=args.repeat * len(args.agents), results=[], passed=False)

    def save():
        report['passed'] = (len(report['results']) == report['planned']
                            and all(row['passed'] for row in report['results']))
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    save()
    for iteration in range(args.repeat):
        for agent in args.agents:
            case = output / f'{agent}-{iteration}'
            case.mkdir()
            guest_root = (args.pi_root or root).resolve() if agent == 'pi' else root
            command = [sys.executable, str(harness),
                       '--agent', agent, '--root', str(guest_root), '--dist', str(dist),
                       '--output', str(case), '--cli', getattr(args, agent), '--timeout', str(args.timeout),
                       '--memory-limit-mb', str(args.memory_limit_mb)]
            if agent == 'pi':
                command.extend(['--node', args.node])
                if args.tool_bin:
                    command.extend(['--tool-bin', args.tool_bin])
            elif args.mcp:
                command.append('--mcp')
            row = dict(agent=agent, iteration=iteration, passed=False)
            started = time.monotonic()
            try:
                with (case / 'runner.stdout.log').open('wb') as stdout, (case / 'runner.stderr.log').open('wb') as stderr:
                    child = SessionProcess(command, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                           memory_limit_bytes=args.memory_limit_mb * 1024**2)
                    try:
                        row['exit_code'] = child.process.wait(timeout=args.timeout + 120)
                    except subprocess.TimeoutExpired:
                        row['timed_out'] = True
                        child.close()
                        child.process.wait(timeout=10)
                    finally:
                        row['memory_metrics'] = child.memory_metrics()
                        child.close()
                        child.process.wait(timeout=10)
                        child.release_process_handle()
                if (case / 'report.json').is_file():
                    result = json.loads((case / 'report.json').read_text(encoding='utf-8'))
                    row.update(passed=row.get('exit_code') == 0 and result['passed'],
                               harness_sha256=result['harness_sha256'],
                               completed=len(result['results']), requests=result['requests'],
                               provider_errors=result['provider_errors'])
            except Exception as error:
                row['error'] = str(error)
            row['seconds'] = time.monotonic() - started
            report['results'].append(row)
            save()
            print(json.dumps(row), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
