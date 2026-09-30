"""Pair cached and uncached shared mutex runs from one frozen native build."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--iterations', type=int, default=100_000)
    parser.add_argument('--pairs', type=int, default=5)
    args = parser.parse_args()
    if args.iterations <= 0 or args.pairs <= 0:
        parser.error('iterations and pairs must be positive')
    folder = args.native.resolve(strict=True)
    manifest = json.loads((folder / 'manifest.json').read_text(encoding='utf-8'))
    executable = folder / manifest['executables']['kinakaze_libpthread']
    rows = []
    for pair in range(args.pairs + 1):
        for enabled in ((0, 1) if pair % 2 == 0 else (1, 0)):
            environment = dict(os.environ, PATH=str(folder) + os.pathsep + os.environ['PATH'],
                               KINAKAZE_PTHREAD_SHARED_CACHE_OPT=str(enabled),
                               KINAKAZE_BENCH_ITERATIONS=str(args.iterations))
            result = subprocess.run([str(executable), '--exact', 'shared::tests::benchmark_shared_mutex_cache',
                                     '--ignored', '--nocapture', '--test-threads=1'],
                                    env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    text=True, encoding='utf-8', errors='replace', timeout=60, check=True)
            match = re.search(r'SHARED_MUTEX_BENCH iterations=(\d+) elapsed_ns=(\d+) cached=(true|false)',
                              result.stdout)
            if not match or int(match[1]) != args.iterations or (match[3] == 'true') != bool(enabled):
                raise RuntimeError('native benchmark record does not match requested configuration:\n' + result.stdout)
            row = dict(pair=pair, warmup=pair == 0, cached=bool(enabled),
                       iterations=int(match[1]), elapsed_ns=int(match[2]))
            rows.append(row)
            print(json.dumps(row), flush=True)
    control = [r['elapsed_ns'] / r['iterations'] for r in rows if not r['warmup'] and not r['cached']]
    candidate = [r['elapsed_ns'] / r['iterations'] for r in rows if not r['warmup'] and r['cached']]
    report = dict(scope='native shared mutex uncontended lock/unlock; no workload throughput claim',
                  executable_sha256=hashlib.sha256(executable.read_bytes()).hexdigest(),
                  native_manifest=manifest, rows=rows, control_median_ns=statistics.median(control),
                  candidate_median_ns=statistics.median(candidate),
                  speedup=statistics.median(control) / statistics.median(candidate))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2), encoding='utf-8', newline='\n')
    print(json.dumps({name: report[name] for name in ('control_median_ns', 'candidate_median_ns', 'speedup')}))


if __name__ == '__main__':
    main()
