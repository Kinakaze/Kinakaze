"""Stream correlated native/guest I/O records into a complete log and bounded summaries.

Input is a benchmark output directory. No record sampling is used. Durations
are elapsed time, not CPU time; inclusive totals across layers must not be added.
"""
import argparse
from collections import Counter, defaultdict
import csv
import gzip
import hashlib
import json
from pathlib import Path
import re
import struct

HEADER = struct.Struct('<IHH7Qq4I')
UNKNOWN = -(1 << 63)


def records(path):
    with path.open('rb', buffering=1024 * 1024) as source:
        while header := source.read(HEADER.size):
            if len(header) != HEADER.size:
                raise ValueError(f'{path.name}: truncated header')
            size, op_length, flags, seq, parent, start, elapsed, a, b, c, result, pid, tid, path_length, version = HEADER.unpack(header)
            if version != 1 or size != HEADER.size + op_length + path_length or size > 16384:
                raise ValueError(f'{path.name}: invalid record format/size')
            payload = source.read(size - HEADER.size)
            if len(payload) != size - HEADER.size:
                raise ValueError(f'{path.name}: truncated payload')
            yield (seq, parent, start, elapsed, a, b, c, result, pid, tid, flags,
                   payload[:op_length].decode('utf-8'), payload[op_length:].decode('utf-8'))


class Stats:
    def __init__(self):
        self.count = self.elapsed = self.exclusive = self.maximum = self.bytes = 0
        self.results = Counter()
        self.histogram = Counter()

    def add(self, elapsed, exclusive, result, op):
        self.count += 1
        self.elapsed += elapsed
        self.exclusive += exclusive
        self.maximum = max(self.maximum, elapsed)
        self.histogram[elapsed.bit_length()] += 1
        # Result cardinality stays bounded: successful handles/byte counts are
        # aggregated; retain the actual values in the complete event log.
        if result == UNKNOWN:
            self.results['unknown'] += 1
        elif result < 0:
            self.results[str(result)] += 1
        elif result == 0x103 and op.startswith('native.Nt'):
            self.results['pending'] += 1
        else:
            self.results['success'] += 1
        if op in ('vfs.read', 'vfs.write', 'map.read_once', 'map.write_once') and result > 0:
            self.bytes += result

    def report(self):
        def percentile(fraction):
            seen = 0
            for exponent, count in sorted(self.histogram.items()):
                seen += count
                if seen >= self.count * fraction:
                    return (1 << exponent) / 1000
            return 0
        return dict(count=self.count, elapsed_ms=self.elapsed / 1e6,
                    exclusive_ms=self.exclusive / 1e6, max_ms=self.maximum / 1e6,
                    p50_upper_us=percentile(.5), p95_upper_us=percentile(.95),
                    bytes=self.bytes, results=dict(self.results))


def analyze(root, output):
    output.mkdir(parents=True, exist_ok=False)
    benchmark_path = root / 'report.json'
    benchmark = json.loads(benchmark_path.read_text(encoding='utf-8')) if benchmark_path.exists() else {}
    phases = [(p['phase'], p['start_unix_ns'], p['end_unix_ns']) for p in benchmark.get('phases', [])
              if 'start_unix_ns' in p]
    groups = defaultdict(Stats)
    paths = defaultdict(Stats)
    processes = Counter()
    integrity = dict(records=0, truncated_paths=0, dropped=0, failed_threads=0,
                     files_without_checkpoint=0, unresolved_parent_spans=0,
                     files_without_final_checkpoint=0, process_exit_records=0,
                     negative_exclusive_spans=0, path_summary_overflow=0, parse_errors=[])
    manifest = []
    source_directory = root / 'session/io-trace'
    trace_files = sorted(source_directory.glob('*.ktrace'))
    with gzip.open(output / 'all-events.tsv.gz', 'wt', encoding='utf-8', newline='', compresslevel=1) as complete:
        writer = csv.writer(complete, delimiter='\t', lineterminator='\n')
        writer.writerow(['file', 'pid', 'tid', 'seq', 'parent', 'phase', 'start_unix_ns', 'elapsed_ns',
                         'exclusive_ns', 'op', 'arg0', 'arg1', 'arg2', 'result', 'flags', 'path'])
        for number, path in enumerate(trace_files):
            children = defaultdict(int)
            checkpoint = False
            dropped = failed = 0
            count = 0
            last_op = None
            try:
                for seq, parent, start, elapsed, a, b, c, result, pid, tid, flags, op, pathname in records(path):
                    count += 1
                    last_op = op
                    integrity['process_exit_records'] += int(op == 'process.exit')
                    integrity['records'] += 1
                    processes[pid] += 1
                    if flags & 1:
                        integrity['truncated_paths'] += 1
                    phase = next((name for name, begin, end in phases if begin <= start <= end), 'outside-phases')
                    exclusive = elapsed - children.pop(seq, 0)
                    if exclusive < 0:
                        integrity['negative_exclusive_spans'] += 1
                    exclusive = max(0, exclusive)
                    if parent:
                        children[parent] += elapsed
                    if op == 'trace.checkpoint':
                        checkpoint = True
                        dropped = max(dropped, a)
                        failed = max(failed, b)
                    groups[phase, op].add(elapsed, exclusive, result, op)
                    if pathname:
                        key = (phase, op, pathname)
                        if key not in paths and len(paths) >= 20000:
                            integrity['path_summary_overflow'] += 1
                        else:
                            paths[key].add(elapsed, exclusive, result, op)
                    writer.writerow([path.name, pid, tid, seq, parent, phase, start, elapsed, exclusive,
                                     op, a, b, c, result, flags, pathname])
            except (ValueError, UnicodeError) as error:
                integrity['parse_errors'].append(str(error))
            integrity['files_without_checkpoint'] += int(not checkpoint)
            integrity['files_without_final_checkpoint'] += int(last_op != 'trace.checkpoint')
            integrity['dropped'] += dropped
            integrity['failed_threads'] += int(failed != 0)
            integrity['unresolved_parent_spans'] += len(children)
            with path.open('rb') as source:
                digest = hashlib.file_digest(source, 'sha256').hexdigest()
            manifest.append(dict(file=path.name, bytes=path.stat().st_size, records=count, sha256=digest))
            if number % 250 == 0:
                print(f'analyzed {number + 1}/{len(trace_files)} files, {integrity["records"]} records', flush=True)
    operations = [dict(phase=phase, op=op, **stats.report()) for (phase, op), stats in groups.items()]
    operations.sort(key=lambda row: (row['phase'], -row['exclusive_ms']))
    top_paths = [dict(phase=phase, op=op, path=path, **stats.report()) for (phase, op, path), stats in paths.items()]
    top_paths.sort(key=lambda row: -row['elapsed_ms'])
    fork = defaultdict(lambda: dict(count=0, total=0, maximum=0))
    for path in source_directory.glob('kinakaze-fork-*.log'):
        for line in path.read_text(encoding='utf-8').splitlines():
            if 'fork timings ' in line:
                for name, value in re.findall(r'(\w+)=(\d+)us', line):
                    stat = fork[name]
                    stat['count'] += 1
                    stat['total'] += int(value)
                    stat['maximum'] = max(stat['maximum'], int(value))
    report = dict(benchmark=benchmark, integrity=integrity, process_count=len(processes),
                  trace_files=len(trace_files), trace_bytes=sum(f['bytes'] for f in manifest),
                  operations=operations, top_paths=top_paths[:200], fork_microseconds=dict(fork), files=manifest,
                  notes=['Inclusive durations overlap; do not add layers or interpret elapsed as CPU.',
                         'Exclusive duration subtracts directly nested events, including trace.flush.',
                         'Logging overhead outside trace.flush remains; compare speed with an untraced run.',
                         'Negative native statuses include expected missing files/EAs, not just defects.',
                         'Native pending status is a submission; completion and waits are separate records.',
                         'Histogram percentile values are power-of-two upper bounds.',
                         'Only instrumented VFS/API boundaries are covered; allocator-frozen fork is suppressed.',
                         'Per-thread termination/forced-kill may lose the final buffer; inspect checkpoints.',
                         'process_count is unique native PID count; Windows can reuse PIDs. Job accounting includes exited processes.',
                         'Top-path summary is bounded; the complete event log retains every decoded path.'])
    (output / 'summary.json').write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')
    lines = ['# I/O trace analysis', '', f'Records: {integrity["records"]:,}; processes: {len(processes)}; '
             f'raw bytes: {report["trace_bytes"]:,}.', '', 'Integrity: `' + json.dumps(integrity) + '`', '',
             '| Phase | Operation | Count | Inclusive ms | Exclusive ms | Max ms |',
             '| --- | --- | ---: | ---: | ---: | ---: |']
    for row in operations:
        lines.append(f'| {row["phase"]} | {row["op"]} | {row["count"]} | {row["elapsed_ms"]:.2f} | '
                     f'{row["exclusive_ms"]:.2f} | {row["max_ms"]:.2f} |')
    lines += ['', *('- ' + note for note in report['notes'])]
    (output / 'summary.md').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = analyze(args.input.resolve(), args.output.resolve())
    print(json.dumps({key: report[key] for key in ('integrity', 'trace_files', 'trace_bytes', 'process_count')}))
    return int(bool(report['integrity']['parse_errors']) or not report['trace_files'])


if __name__ == '__main__':
    raise SystemExit(main())
