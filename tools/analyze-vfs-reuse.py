"""Attribute existing I/O events to callers and summarize reusable image reads.

Reads analyze-io-trace.py's complete event log; never launches a guest workload.
Span records are emitted on completion, so only unfinished ancestors are kept.
"""
import argparse
from collections import defaultdict
import csv
import gzip
import json
from pathlib import Path
import re


OPERATIONS = {
    'native.CreateFileW', 'native.NtCreateFile', 'native.NtQueryAttributesFile',
    'native.NtQueryEaFile', 'map.open_metadata', 'map.resolve',
}


def add(table, key, count, elapsed):
    value = table.setdefault(key, [0, 0])
    value[0] += count
    value[1] += elapsed


def callers(events, phase):
    pending = {}
    direct, ancestors, results = {}, {}, {}
    accounts = {}
    current = None
    unresolved = 0
    with gzip.open(events, 'rt', encoding='utf-8', newline='') as source:
        for row in csv.DictReader(source, delimiter='\t'):
            if current != row['file']:
                unresolved += len(pending)
                pending.clear()
                current = row['file']
            seq, parent = int(row['seq']), int(row['parent'])
            operation = row['op']
            nested, immediate = pending.pop(seq, ({}, {}))
            for name, (count, elapsed) in nested.items():
                add(ancestors, (operation, name), count, elapsed)
            for name, (count, elapsed) in immediate.items():
                add(direct, (operation, name), count, elapsed)
            selected = row['phase'] == phase
            if selected and operation == 'map.resolve' and row['path'] in (
                    '/etc/passwd', '/etc/group'):
                key = (row['file'], row['pid'], row['tid'], row['path'])
                add(accounts, key, 1, int(row['elapsed_ns']))
            if selected and operation in ('vfs.rmdir', 'vfs.unlink', 'vfs.rename'):
                add(results, (operation, row['result']), 1, int(row['elapsed_ns']))
            if parent and (nested or selected and operation in OPERATIONS):
                into, child = pending.setdefault(parent, ({}, {}))
                for name, (count, elapsed) in nested.items():
                    add(into, name, count, elapsed)
                if selected and operation in OPERATIONS:
                    elapsed = int(row['elapsed_ns'])
                    add(into, operation, 1, elapsed)
                    add(child, operation, 1, elapsed)
    unresolved += len(pending)

    def rows(table, keys):
        return [dict(zip(keys, key), count=count, elapsed_ms=elapsed / 1e6)
                for key, (count, elapsed) in sorted(table.items(), key=lambda p: -p[1][1])]

    return dict(phase=phase, unresolved_ancestors=unresolved,
                direct=rows(direct, ('parent', 'operation')),
                ancestors=rows(ancestors, ('ancestor', 'operation')),
                results=rows(results, ('operation', 'result')),
                account_queries=rows(accounts, ('file', 'pid', 'tid', 'path')))


def startup(directory, report):
    """Group spans by their start time, retaining unmatched and idle spans too."""
    windows = [] if report is None else json.loads(report.read_text(encoding='utf-8'))['phases']
    groups = {}
    matched = 0
    for path in directory.glob('startup-*.log'):
        with path.open(encoding='utf-8') as source:
            for line in source:
                fields = dict(re.findall(r'(\w+)=(\S+)', line))
                required = ('phase', 'start_us', 'wall_us', 'thread_cpu_us', 'process_cpu_us')
                if not all(key in fields for key in required):
                    continue
                start = int(fields['start_us']) * 1000
                phase = next((p['phase'] for p in windows
                              if p['start_unix_ns'] <= start < p['end_unix_ns']), 'unattributed')
                value = groups.setdefault((phase, fields['phase']), [0, 0, 0, 0])
                value[0] += 1
                for index, field in enumerate(required[2:], 1):
                    value[index] += int(fields[field])
                matched += 1
    return dict(records=matched, spans=[
        dict(phase=phase, operation=op, count=count, wall_ms=wall / 1000,
             thread_cpu_ms=thread / 1000, process_cpu_ms=process / 1000)
        for (phase, op), (count, wall, thread, process) in
        sorted(groups.items(), key=lambda item: (item[0][0], -item[1][1]))])


def images(directory):
    operations, paths, execution = {}, {}, defaultdict(lambda: [0, 0, 0])
    for path in directory.glob('loader-*.log'):
        with path.open(encoding='utf-8') as source:
            for line in source:
                match = re.match(r'(\S+) elapsed_us=(\d+).* object=(.*)', line)
                if not match:
                    continue
                operation, elapsed, name = match[1], int(match[2]), match[3]
                add(operations, operation, 1, elapsed)
                if operation == 'image-snapshot':
                    add(paths, name, 1, elapsed)
    for path in directory.glob('execution-*.log'):
        with path.open(encoding='utf-8') as source:
            for line in source:
                match = re.match(r'(\S+) elapsed_us=(\d+) bytes=(\d+)', line)
                if match:
                    value = execution[match[1]]
                    value[0] += 1
                    value[1] += int(match[2])
                    value[2] += int(match[3])
    snapshots = []
    for path, (count, elapsed) in sorted(paths.items(), key=lambda p: -p[1][0]):
        try:
            # Current file length is explicitly not a historical log field.
            length = Path(path).stat().st_size
        except OSError:
            length = None
        snapshots.append(dict(path=path, count=count, elapsed_us=elapsed,
                              current_size=length))
    return dict(scope='all logged phases; loader records have no timestamps',
                loader_operations={name: dict(count=count, elapsed_us=elapsed)
                                   for name, (count, elapsed) in operations.items()},
                execution_operations={name: dict(count=count, elapsed_us=elapsed, bytes=size)
                                      for name, (count, elapsed, size) in execution.items()},
                snapshot_paths=snapshots)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--events', required=True, type=Path)
    parser.add_argument('--profile', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--phase', default='install')
    parser.add_argument('--report', type=Path, help='Benchmark report with phase UTC boundaries')
    args = parser.parse_args()
    result = dict(callers=callers(args.events, args.phase), images=images(args.profile),
                  startup=startup(args.profile, args.report),
                  notes=['Inclusive ancestor costs overlap; never sum them across layers.',
                         'Startup phase attribution uses span start time; spans can cross boundaries.',
                         'Startup CPU values have OS timer granularity; process CPU includes other threads.',
                         'Idle init-pool-wait spans overlap work and are not install latency.',
                         'This analyzes existing logs and does not measure optimized code.'])
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n',
                           encoding='utf-8')
    print(f"Wrote {args.output}; unresolved ancestors: "
          f"{result['callers']['unresolved_ancestors']}")


if __name__ == '__main__':
    main()
