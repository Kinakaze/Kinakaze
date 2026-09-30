"""Analyze existing traces only: exact disk-backed paths and nearest-call attribution.

Does not launch the guest or modify a guest filesystem. Each native event is
assigned to its nearest VFS boundary (or its recorded root when no VFS boundary
exists), so nested open/openat wrappers do not double-count the event.
"""
import argparse
from bisect import bisect_right
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
import heapq
import importlib.util
import json
from pathlib import Path
import re
import sqlite3

spec = importlib.util.spec_from_file_location('trace_reader', Path(__file__).with_name('analyze-io-trace.py'))
reader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reader)


def combine(target, source):
    for key, (count, elapsed) in source.items():
        if key in target:
            target[key][0] += count
            target[key][1] += elapsed
        else:
            target[key] = [count, elapsed]


def add(target, key, count, elapsed):
    values = target[key]
    values[0] += count
    values[1] += elapsed


def table(values, labels):
    rows = [dict(zip(labels, key if isinstance(key, tuple) else (key,)),
                 count=value[0], elapsed_ms=value[1] / 1e6) for key, value in values.items()]
    return sorted(rows, key=lambda row: -row['elapsed_ms'])


def merge_intervals(intervals):
    result = []
    for start, end in sorted(intervals):
        if result and start <= result[-1][1]:
            result[-1][1] = max(result[-1][1], end)
        else:
            result.append([start, end])
    return result


def interval_size(intervals):
    return sum(end - start for start, end in intervals)


def intersection(a, b):
    i = j = total = 0
    while i < len(a) and j < len(b):
        total += max(0, min(a[i][1], b[j][1]) - max(a[i][0], b[j][0]))
        if a[i][1] < b[j][1]:
            i += 1
        else:
            j += 1
    return total


def size_bucket(size):
    for ceiling in (0, 64, 512, 4096, 16384, 65536, 131072, 1048576):
        if size <= ceiling:
            return str(ceiling)
    return '>1MiB'


def packages(guest_root):
    starts, finished = [], {}
    for line in (guest_root / 'var/log/dpkg.log').read_text(encoding='utf-8').splitlines():
        day, clock, *event = line.split()
        stamp = int(datetime.fromisoformat(day + 'T' + clock).replace(tzinfo=timezone(timedelta(hours=8))).timestamp() * 1e9)
        if event[0] == 'install':
            starts.append((stamp, event[1]))
        if event[:2] == ['status', 'unpacked']:
            # Configuration logs 'unpacked' again; retain the unpack completion.
            finished.setdefault(event[2], stamp)
    # The guest path encoder maps ':' to the private-use U+F03A on NTFS.
    listings = {p.stem.replace('\uf03a', ':'): p for p in (guest_root / 'var/lib/dpkg/info').glob('*.list')}
    result = []
    for start, package in starts:
        listing = listings.get(package) or listings.get(package.split(':')[0])
        entries = listing.read_text(encoding='utf-8').splitlines() if listing else []
        result.append(dict(package=package, start_unix_ns=start,
                           unpack_log_seconds=(finished.get(package, start) - start) / 1e9,
                           list_entries=len(entries)))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root, output = args.input.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    benchmark = json.loads((root / 'report.json').read_text(encoding='utf-8'))
    install = next(p for p in benchmark['phases'] if p['phase'] == 'install')
    begin, end = install['start_unix_ns'], install['end_unix_ns']
    package_rows = packages(Path(benchmark['root']))
    package_starts = [row['start_unix_ns'] for row in package_rows]
    package_native = defaultdict(lambda: [0, 0])
    db = sqlite3.connect(output / 'paths.sqlite3')
    db.execute('PRAGMA journal_mode=OFF')
    db.execute('PRAGMA cache_size=-8192')
    db.execute('CREATE TABLE paths(op TEXT,path TEXT,result INTEGER,calls INTEGER,ns INTEGER,self_ns INTEGER, PRIMARY KEY(op,path,result)) WITHOUT ROWID')
    path_batch = {}

    def flush_paths():
        db.executemany('INSERT INTO paths VALUES(?,?,?,?,?,?) ON CONFLICT(op,path,result) DO UPDATE SET calls=calls+excluded.calls,ns=ns+excluded.ns,self_ns=self_ns+excluded.self_ns',
                       ((*key, *value) for key, value in path_batch.items()))
        db.commit()
        path_batch.clear()

    nearest, direct = defaultdict(lambda: [0, 0]), defaultdict(lambda: [0, 0])
    scopes, io_groups = defaultdict(lambda: [0, 0]), {}
    native_totals = defaultdict(lambda: [0, 0])
    errors = defaultdict(lambda: [0, 0])
    executions = Counter()
    wait_intervals, fork_intervals = [], []
    thread_rows, slow = [], []
    negative_exclusive = unresolved = processed = 0
    for number, file in enumerate(sorted((root / 'session/io-trace').glob('*.ktrace'))):
        # Pending parent state is bounded by active nesting, not file size.
        pending = {}
        thread = dict(file=file.name, records=0, native={}, first=end, last=begin, root_ns=0, exec_targets=[])
        for seq, parent, start, elapsed, a, b, c, result, pid, tid, flags, op, path in reader.records(file):
            if not begin <= start <= end:
                continue
            processed += 1
            thread['records'] += 1
            thread['pid'], thread['tid'] = pid, tid
            thread['first'] = min(thread['first'], start)
            thread['last'] = max(thread['last'], start + elapsed)
            children_ns, metrics, children_native, regular = pending.pop(seq, (0, {}, {}, False))
            exclusive = elapsed - children_ns
            negative_exclusive += int(exclusive < 0)
            for (native_op, native_result), value in children_native.items():
                add(direct, (op, native_op, native_result), *value)
            if result < 0 and result != reader.UNKNOWN:
                add(errors, (op, result), 1, elapsed)
            if path:
                key = (op, path, result)
                if key in path_batch:
                    value = path_batch[key]
                    value[0] += 1
                    value[1] += elapsed
                    value[2] += max(0, exclusive)
                else:
                    path_batch[key] = [1, elapsed, max(0, exclusive)]
                if len(path_batch) >= 20000:
                    flush_paths()
            if op.startswith('native.'):
                metrics[op] = [1, elapsed]
                add(native_totals, op, 1, elapsed)
                combine(thread['native'], {op: [1, elapsed]})
                index = bisect_right(package_starts, start) - 1
                package = package_rows[index]['package'] if index >= 0 else '(before-first-install-record)'
                add(package_native, (package, op), 1, elapsed)
                if op in ('native.WaitForMultipleObjects', 'native.WaitForSingleObject', 'native.flush_completion'):
                    wait_intervals.append((start, start + elapsed))
            if op in ('map.ea_read', 'map.read_once', 'map.write_once'):
                regular = True
            if op == 'process.fork':
                fork_intervals.append((start, start + elapsed))
            if op == 'process.exec':
                executions[path] += 1
                thread['exec_targets'].append(path)
            if op.startswith('vfs.') or parent == 0:
                owner = op if not op.startswith('native.') else '(unscoped native)'
                for native_op, value in metrics.items():
                    add(nearest, (owner, native_op), *value)
                wait = sum(v[1] for k, v in metrics.items() if 'WaitFor' in k)
                add(scopes, (owner, 'with-regular-file-metadata' if regular else 'without-regular-file-metadata'), 1, elapsed)
                if op in ('vfs.read', 'vfs.write'):
                    key = (op, a, size_bucket(b), 'regular-metadata' if regular else 'no-regular-metadata')
                    row = io_groups.setdefault(key, dict(calls=0, elapsed_ns=0, wait_ns=0, requested=0, returned=0, zero=0, interrupted=0))
                    row['calls'] += 1
                    row['elapsed_ns'] += elapsed
                    row['wait_ns'] += wait
                    row['requested'] += b
                    row['returned'] += max(0, result)
                    row['zero'] += int(result == 0)
                    row['interrupted'] += int(result == -4)
                if wait > 10_000_000:
                    item = (wait, number, seq, dict(file=file.name, op=op, start_unix_ns=start, duration_ms=elapsed / 1e6,
                            wait_ms=wait / 1e6, arg0=a, arg1=b, result=result, path=path, regular_metadata=regular))
                    if len(slow) < 60:
                        heapq.heappush(slow, item)
                    elif item[:3] > slow[0][:3]:
                        heapq.heapreplace(slow, item)
                metrics = {}
            if parent:
                state = pending.setdefault(parent, [0, {}, {}, False])
                state[0] += elapsed
                combine(state[1], metrics)
                if op.startswith('native.'):
                    key = (op, result)
                    if key in state[2]:
                        state[2][key][0] += 1
                        state[2][key][1] += elapsed
                    else:
                        state[2][key] = [1, elapsed]
                state[3] |= regular
            else:
                thread['root_ns'] += elapsed
        unresolved += len(pending)
        if thread['records']:
            thread_rows.append(thread)
        if number % 500 == 0:
            print(f'{number + 1} files; {processed:,} install events; pending integrity={unresolved}', flush=True)
    flush_paths()
    # Unlike the first summary, these queries cover all path keys on disk.
    db.row_factory = sqlite3.Row
    path_queries = {
        'most_repeated': 'SELECT op,path,SUM(calls) calls,SUM(ns)/1e6 elapsed_ms FROM paths GROUP BY op,path ORDER BY calls DESC LIMIT 100',
        'most_expensive': 'SELECT op,path,SUM(calls) calls,SUM(ns)/1e6 elapsed_ms FROM paths GROUP BY op,path ORDER BY SUM(ns) DESC LIMIT 100',
        'negative_by_operation': f'SELECT op,result,SUM(calls) calls,SUM(ns)/1e6 elapsed_ms,COUNT(*) distinct_paths FROM paths WHERE result<0 AND result!={reader.UNKNOWN} GROUP BY op,result ORDER BY SUM(ns) DESC',
        'account_databases': "SELECT op,path,result,calls,ns/1e6 elapsed_ms FROM paths WHERE path IN ('/etc/passwd','/etc/group','/etc/nsswitch.conf') ORDER BY ns DESC",
        'cleanup_patterns': "SELECT op,result,CASE WHEN path LIKE '%dpkg-new%' THEN 'dpkg-new' WHEN path LIKE '%dpkg-tmp%' THEN 'dpkg-tmp' WHEN path LIKE '%dpkg-old%' THEN 'dpkg-old' ELSE 'other' END pattern,SUM(calls) calls,SUM(ns)/1e6 elapsed_ms FROM paths WHERE op IN ('vfs.rmdir','vfs.rename','vfs.lstat','vfs.unlink') GROUP BY op,result,pattern ORDER BY SUM(ns) DESC",
        'path_rows': 'SELECT COUNT(*) rows,SUM(calls) events FROM paths',
    }
    path_results = {name: [dict(row) for row in db.execute(query)] for name, query in path_queries.items()}
    db.close()
    attributed = defaultdict(lambda: [0, 0])
    for (_, native_op), value in nearest.items():
        add(attributed, native_op, *value)
    assert dict(attributed) == dict(native_totals), 'native attribution lost/doubled events'
    wait_union, fork_union = merge_intervals(wait_intervals), merge_intervals(fork_intervals)
    startup = defaultdict(lambda: [0, 0])
    startup_values = defaultdict(list)
    for file in (root / 'session/profile').glob('startup-*.log'):
        for line in file.read_text(encoding='utf-8').splitlines():
            values = dict(re.findall(r'(\w+)=([^ ]+)', line))
            if 'start_us' in values and begin <= int(values['start_us']) * 1000 <= end:
                phase, ns = values['phase'], int(values['wall_us']) * 1000
                add(startup, phase, 1, ns)
                startup_values[phase].append(ns)
    startup_rows = table(startup, ('phase',))
    for row in startup_rows:
        values = sorted(startup_values[row['phase']])
        row.update(p50_ms=values[len(values) // 2] / 1e6, p95_ms=values[min(len(values)-1, int(len(values)*.95))] / 1e6)
    resources = [json.loads(line) for line in (root / 'install/resources.jsonl').read_text(encoding='utf-8').splitlines()]
    lifetimes = {}
    for sample in resources:
        for process in sample['processes']:
            key = (process['pid'], process['born'])
            row = lifetimes.setdefault(key, dict(pid=process['pid'], born=process['born'], samples=0, first=sample['unix_ns'], last=sample['unix_ns'],
                   handles_first=process['handles'], handles_last=process['handles'], handles_min=process['handles'], handles_max=process['handles'],
                   private_first=process['private'], private_last=process['private'], private_min=process['private'], private_max=process['private'], cpu_last=process['cpu']))
            row.update(last=sample['unix_ns'], samples=row['samples']+1, handles_last=process['handles'], private_last=process['private'], cpu_last=process['cpu'])
            for metric in ('handles', 'private'):
                row[metric+'_min'] = min(row[metric+'_min'], process[metric])
                row[metric+'_max'] = max(row[metric+'_max'], process[metric])
    report = dict(install=install, integrity=dict(events=processed, unresolved_parents=unresolved, negative_exclusive=negative_exclusive, native_attribution_exact=True),
        nearest_native=table(nearest, ('owner', 'native')), direct_native=table(direct, ('parent', 'native', 'result')),
        errors=table(errors, ('op','result')), paths=path_results, startup=startup_rows,
        exec_targets=dict(executions.most_common()), package_windows=package_rows, package_native=table(package_native, ('package','native')),
        threads=sorted(thread_rows, key=lambda row: -row['root_ns']), slow_wait_scopes=[row[3] for row in sorted(slow,reverse=True)],
        io=[dict(op=key[0],fd=key[1],request_ceiling=key[2],classification=key[3],**value) for key,value in io_groups.items()],
        overlap=dict(wait_sum_ms=sum(b-a for a,b in wait_intervals)/1e6,wait_union_ms=interval_size(wait_union)/1e6,
                     fork_sum_ms=sum(b-a for a,b in fork_intervals)/1e6,fork_union_ms=interval_size(fork_union)/1e6,
                     wait_fork_intersection_ms=intersection(wait_union,fork_union)/1e6),
        sampled_processes=sorted(lifetimes.values(),key=lambda row:-row['samples']),
        notes=['Installation only; this is the frozen pre-optimization candidate-io1 trace.',
               'Intervals use UTC start + monotonic duration, assuming no system clock jumps during this run.',
               'Wait union is covered wall time, not proof of an idle CPU or a recoverable speedup.',
               'Nearest VFS attribution is exact for the recorded tree; uninstrumented callers remain unscoped.',
               'Regular metadata classification is an observed descendant signature, not a recorded descriptor type.',
               'dpkg package windows have one-second resolution and can include adjacent preparation work; same-second starts make per-package native attribution ambiguous.',
               'Resource samples cover live processes once a second, missing many short-lived processes.',
               'Exec markers describe requested targets in the calling process, not an authoritative child argv.',
               'Startup nested phases overlap fork/bootstrap intervals; never add all totals.'])
    (output / 'deep-summary.json').write_text(json.dumps(report,indent=2,ensure_ascii=False),encoding='utf-8')
    print(json.dumps(dict(integrity=report['integrity'],overlap=report['overlap'],path_rows=path_results['path_rows'])),flush=True)


if __name__ == '__main__':
    main()
