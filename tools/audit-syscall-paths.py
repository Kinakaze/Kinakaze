"""Inventory every Linux 6.12 x86-64 syscall and its required path review.

Static routing and source references are evidence of implementation only. They
never promote a syscall, path, benchmark or compatibility claim to verified.
"""
import argparse
import csv
from datetime import datetime, timezone
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re

AXES = {
    'entry': 'raw/libc entry, JIT/AOT bridge, tracing and register preservation',
    'validation': 'pointer/layout/length/flags/subcommand validation and errno order',
    'normal': 'normal, empty, boundary, partial and vector operations',
    'waiting': 'sync/async, blocking/nonblocking, readiness, timeout and signals',
    'ownership': 'private/shared, aliases, fd/object generation and credentials',
    'concurrency': 'atomicity, wake ordering, cancellation and concurrent mutation',
    'lifecycle': 'thread exit, process death, fork, exec and retained resources',
    'fallback': 'errors, unsupported requests, disabled switches and host fallback',
    'compatibility': 'Linux ABI, permissions, subcommands and observable side effects',
    'performance': 'per-syscall/path control measurements and guest end-to-end results',
}
TECHNIQUES = ['jit', 'shared-memory', 'assembly', 'avx-simd', 'event-driven',
              'switchable-pruning', 'template-generation']


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_audit():
    path = Path(__file__).with_name('audit-syscalls.py')
    spec = importlib.util.spec_from_file_location('kinakaze_syscall_audit', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def inventory(root):
    audit = load_audit().audit(root)
    table = root / 'docs/architecture-v2-syscalls.csv'
    with table.open(encoding='utf-8-sig', newline='') as stream:
        contracts = {int(row['number']): row for row in csv.DictReader(stream)}
    if len(contracts) != len(audit['syscalls']):
        raise ValueError('syscall contract/dispatch inventory mismatch')
    source = root / audit['source']
    lines = source.read_text(encoding='utf-8').splitlines()
    arm_lines = sorted({row['line'] for row in audit['syscalls'] if row['line']})
    arm_end = {}
    for position, line in enumerate(arm_lines):
        # Find the next dispatch arm or the default arm, excluding any nested
        # match patterns by requiring the original indentation.
        indent = re.match(r'\s*', lines[line - 1])[0]
        end = arm_lines[position + 1] - 1 if position + 1 < len(arm_lines) else len(lines)
        for index in range(line, end):
            if re.match(re.escape(indent) + r'_\s*=>', lines[index]):
                end = index
                break
        arm_end[line] = end
    rows = []
    for routed in audit['syscalls']:
        contract = contracts[routed['number']]
        body = '\n'.join(lines[routed['line'] - 1:arm_end[routed['line']]]) if routed['line'] else ''
        handlers = sorted(set(re.findall(
            r'\b((?:(?:crate|[a-zA-Z_]\w*)::)*(?:kinakaze_abi_\w+|futex_\w+|waitv))\s*\(', body)
            + re.findall(r'\b(futex_(?:requeue::syscall|scalar::wait))\s*\(', body)))
        rows.append(dict(
            **routed,
            owner=contract['owner'],
            design_section=contract['design_section'],
            contract=contract['implementation_plan'],
            linux_validation_focus=contract['validation_focus'],
            handlers=handlers,
            implementation_source=dict(path=audit['source'], line=routed['line']),
            paths={axis: dict(status='pending', requirement=requirement,
                              evidence=[]) for axis, requirement in AXES.items()},
            techniques={name: dict(status='unevaluated', evidence=[]) for name in TECHNIQUES},
            completion='unproven',
        ))
    tracked = [table, source,
               root / 'tools/audit-syscalls.py',
               root / 'tools/audit-syscall-paths.py',
               root / 'engine/crates/guest-engine/src/execution/instruction_trampoline.rs',
               root / 'libs/libc/src/futex.rs',
               root / 'libs/libc/src/sysadmin/futex_vector.rs',
               root / 'libs/libc/src/sysadmin/futex_requeue.rs',
               root / 'libs/libc/src/sysadmin/futex_scalar.rs',
               root / 'libs/libc/src/futex/hybrid.rs',
               root / 'libs/libc/src/futex/wait_group.rs',
               root / 'engine/crates/kinakaze-vfs/src/lib.rs',
               root / 'engine/crates/kinakaze-vfs/src/tmpfs/read_pages.rs']
    return dict(
        schema=1,
        generated_at=datetime.now(timezone.utc).isoformat(),
        scope='All entries in the checked-in Linux 6.12 x86-64 LP64 table; all path reviews remain required.',
        evidence_policy='Dispatch and common bridge optimizations do not prove syscall semantics, path completeness or speedups.',
        source_sha256={str(path.relative_to(root)): digest(path) for path in tracked},
        counts={**audit['counts'], 'syscalls': len(rows),
                'required_path_reviews': len(rows) * len(AXES),
                'verified_path_reviews': 0},
        common_implementation=[
            dict(path='JIT/AOT syscall bridge templates',
                 source='engine/crates/guest-engine/src/execution/instruction_trampoline.rs',
                 switch='KINAKAZE_SYSCALL_TEMPLATE', status='implemented',
                 limits='Segment-local unpublished code; runtime VEH and return-breakpoint paths use their existing generators.'),
            dict(path='raw/libc syscall dispatch and ptrace gate', source=audit['source'],
                 status='implemented', limits='Per-syscall ABI, policy, error and trace checks remain required.'),
            dict(path='VFS/Unix read/write descriptor routing',
                 source='engine/crates/kinakaze-vfs/src/lib.rs', switch='KINAKAZE_IO_ROUTE_OPT',
                 status='implemented', limits='Read/write routing only; no evidence of Unix throughput gain or other operation speedups.'),
        ],
        syscalls=rows,
    )


def csv_text(report):
    stream = io.StringIO(newline='')
    columns = ['number', 'name', 'status', 'owner', 'source', 'line', 'handlers', *AXES, 'completion']
    writer = csv.DictWriter(stream, fieldnames=columns)
    writer.writeheader()
    for row in report['syscalls']:
        writer.writerow(dict(number=row['number'], name=row['name'], status=row['status'],
                             owner=row['owner'], source=row['implementation_source']['path'],
                             line=row['line'], handlers='; '.join(row['handlers']),
                             **{axis: row['paths'][axis]['status'] for axis in AXES},
                             completion=row['completion']))
    return stream.getvalue()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--csv', type=Path)
    args = parser.parse_args()
    report = inventory(Path(__file__).resolve().parents[1])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    if args.csv:
        args.csv.parent.mkdir(parents=True, exist_ok=True)
        args.csv.write_text(csv_text(report), encoding='utf-8-sig')
    print(json.dumps(report['counts']))


if __name__ == '__main__':
    main()
