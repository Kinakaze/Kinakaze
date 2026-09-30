"""Correlate existing deep trace output; never launches a guest workload."""
import argparse
from collections import defaultdict
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location('deep', Path(__file__).with_name('analyze-io-trace-deep.py'))
deep = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deep)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--analysis', type=Path, required=True)
    args = parser.parse_args()
    report = json.loads((args.analysis / 'deep-summary.json').read_text(encoding='utf-8'))
    begin, end = (report['install'][key] for key in ('start_unix_ns', 'end_unix_ns'))
    # Largest recorded thread, selected from data rather than a reused PID.
    main_thread = max(report['threads'], key=lambda row: row['records'])['file']
    roots = defaultdict(list)
    main_native = defaultdict(list)
    write_waits, read_waits = [], []
    failures, selinux = defaultdict(lambda: [0, 0]), []
    main_roots = defaultdict(lambda: [0, 0])
    unscoped_waits = defaultdict(lambda: [0, 0])
    unknown_parents = 0
    for number, thread in enumerate(report['threads']):
        file = args.input / 'session/io-trace' / thread['file']
        is_main = file.name == main_thread
        pending = {}
        for seq, parent, start, elapsed, a, b, c, result, pid, tid, flags, op, path in deep.reader.records(file):
            if not begin <= start <= end:
                continue
            native, waits = pending.pop(seq, ({}, []))
            if op.startswith('native.'):
                deep.combine(native, {op: [1, elapsed]})
                if is_main:
                    main_native[op].append((start, start + elapsed))
                if 'WaitFor' in op:
                    waits.append((start, start + elapsed))
                    if parent == 0:
                        deep.add(unscoped_waits, (op, result, a, b, c), 1, elapsed)
            if op == 'vfs.write':
                write_waits.extend(waits)
                waits = []
            elif op == 'vfs.read':
                if is_main and a == 8:
                    read_waits.extend(waits)
                waits = []
            if op == 'vfs.stat' and path == '/sys/fs/selinux':
                selinux.append(dict(file=file.name, elapsed_ms=elapsed / 1e6,
                                    result=result, native=native.copy()))
            if parent:
                target = pending.setdefault(parent, ({}, []))
                deep.combine(target[0], native)
                target[1].extend(waits)
            elif is_main:
                roots[op].append((start, start + elapsed))
                deep.add(main_roots, op, 1, elapsed)
                if result == -2 and op in ('vfs.rmdir', 'vfs.rename', 'vfs.lstat'):
                    for native_op, value in native.items():
                        deep.add(failures, (op, native_op), *value)
        unknown_parents += len(pending)
        if number % 1000 == 0:
            print(f'{number + 1} threads', flush=True)
    write_union = deep.merge_intervals(write_waits)
    root_union = deep.merge_intervals([interval for group in roots.values() for interval in group])
    overlap = []
    for op, intervals in roots.items():
        overlap.append(dict(op=op, overlap_ms=deep.intersection(write_union, deep.merge_intervals(intervals)) / 1e6))
    native_overlap = []
    for op, intervals in main_native.items():
        native_overlap.append(dict(op=op, overlap_ms=deep.intersection(write_union, deep.merge_intervals(intervals)) / 1e6))
    summary = dict(main_thread=main_thread, unresolved_parents=unknown_parents,
                   write_wait_sum_ms=sum(b-a for a,b in write_waits)/1e6,
                   write_wait_union_ms=deep.interval_size(write_union)/1e6,
                   main_root_union_ms=deep.interval_size(root_union)/1e6,
                   write_wait_during_main_root_ms=deep.intersection(write_union,root_union)/1e6,
                   write_wait_during_main_fd8_read_wait_ms=deep.intersection(write_union,deep.merge_intervals(read_waits))/1e6,
                   write_wait_overlap_main_roots=sorted(overlap,key=lambda row:-row['overlap_ms']),
                   write_wait_overlap_main_native=sorted(native_overlap,key=lambda row:-row['overlap_ms']),
                   main_roots=deep.table(main_roots,('op',)),
                   main_failed_lookup_native=deep.table(failures,('owner','native')),
                   unscoped_waits=deep.table(unscoped_waits,('op','result','arg0','arg1','arg2')),
                   selinux=selinux,
                   notes=['Overlap measures co-occurrence, not a proof of causality or a pipe endpoint pairing.',
                          'Main thread is selected by event count; PID reuse is separated by trace filename.',
                          'Root and native overlap tables contain different nesting levels and must not be added.'])
    (args.analysis / 'overlap-summary.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in summary.items() if not isinstance(v,list)}),flush=True)


if __name__ == '__main__':
    main()
