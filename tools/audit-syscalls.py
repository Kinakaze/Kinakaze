"""Audit x86-64 dispatcher coverage against the checked-in Linux 6.12 table.

Dispatch coverage is not semantic compatibility. No syscall is executed: invoking
all numbers with dummy pointers would be unsafe (exit, mount, signals, etc.).
"""
import argparse
import csv
import json
from pathlib import Path
import re


def audit(root):
    path = root / 'libs/libc/src/sysadmin.rs'
    source = path.read_text(encoding='utf-8')
    constants = {name: int(value) for name, value in re.findall(
        r'const (SYS_\w+): i64 = (\d+);', source)}
    entry = source.find('fn dispatch_syscall(')
    if entry < 0:
        entry = source.index('fn kinakaze_abi_syscall_raw(')
    match_start = source.index('let res = match number {', entry)
    start = source.rfind('\n', 0, match_start) + 1
    indent = source[start:match_start]
    end = source.index('\n' + indent + '};', match_start)
    block = source[start:end]
    token = r'(?:SYS_\w+|\d+)'
    span = token + r'(?:\.\.=' + token + r')?'
    arm_indent = indent + '    '
    arms = list(re.finditer(r'^' + re.escape(arm_indent) + '(' + span + r'(?:\s*\|\s*' + span + r')*)\s*=>', block, re.M))
    def value(token):
        return constants[token] if token.startswith('SYS_') else int(token)
    dispatch = {}
    for index, arm in enumerate(arms):
        body = block[arm.end():arms[index + 1].start() if index + 1 < len(arms) else block.index('\n' + arm_indent + '_ =>')]
        line = source.count('\n', 0, start + arm.start()) + 1
        for token in arm[1].split('|'):
            token = token.strip()
            if '..=' in token:
                lo, hi = map(value, token.split('..='))
                numbers = range(lo, hi + 1)
            else:
                numbers = [value(token)]
            for number in numbers:
                if number in dispatch:
                    raise ValueError(f'duplicate syscall dispatch {number} at lines {dispatch[number]["line"]} and {line}')
                dispatch[number] = dict(line=line, status='dispatched', direct_errno=None)
                refusal = re.fullmatch(r'\s*-i64::from\((\w+)\),?\s*', body)
                if refusal:
                    dispatch[number].update(status='explicit-refusal', direct_errno=refusal[1])
    rows = []
    with (root / 'docs/architecture-v2-syscalls.csv').open(encoding='utf-8-sig', newline='') as stream:
        for entry in csv.DictReader(stream):
            number = int(entry['number'])
            rows.append(dict(number=number, name=entry['name'], **dispatch.get(number, dict(
                status='missing-dispatch', line=None, direct_errno='ENOSYS'))))
    return dict(scope=__doc__, source=str(path.relative_to(root)),
                counts={status: sum(row['status'] == status for row in rows)
                        for status in ('dispatched', 'explicit-refusal', 'missing-dispatch')}, syscalls=rows)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = audit(Path(__file__).resolve().parents[1])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report['counts']))
    for row in report['syscalls']:
        if row['status'] != 'dispatched':
            print(row['number'], row['name'], row['status'], row['direct_errno'])
