"""Measure validated daily workloads using one persistent init per distribution.

Each sample runs in a fresh worker. Timings include Bash and child processes;
they are not isolated syscall timings or measurements of cold filesystem caches.
"""
import argparse
from contextlib import ExitStack
import json
import math
from pathlib import Path
import random
import shlex
import statistics
import uuid

from init_pool import InitPool, distribution_hashes


CASES = {
    'spawn': 'for i in {1..20}; do /bin/true; done',
    'text': "seq 1 10000 > numbers; sort -rn numbers > sorted; test \"$(head -n1 sorted)\" = 10000; test \"$(awk '{s+=$1} END {print s}' numbers)\" = 50005000; sed -n '100p' numbers | grep -qx 100",
    'files': 'mkdir tree; for i in {1..50}; do printf data > tree/f$i; done; cp -r tree copy; test "$(find copy -type f | wc -l)" -eq 50; cmp tree/f25 copy/f25; stat copy/f25 > /dev/null',
    'archive': 'mkdir tree; for i in {1..30}; do printf archive-data > tree/f$i; done; tar -czf data.tar.gz tree; mkdir extracted; tar -xzf data.tar.gz -C extracted; diff -r tree extracted/tree',
    'process': 'ps -eo pid,ppid,comm > processes; grep -q bash processes; cat /proc/self/status > status; grep -q Name: status',
    'package-query': 'dpkg-query -W bash > packages; grep -q bash packages',
    'python': "python3 -c 'import json, pathlib, hashlib; p=pathlib.Path(\"data.json\"); p.write_text(json.dumps(list(range(10000)))); a=json.loads(p.read_text()); assert sum(a)==49995000; assert len(hashlib.sha256(p.read_bytes()).digest())==32'",
    'pipeline': "seq 1 20000 | awk '$1 % 2 == 0' | sort -rn | tee even | wc -l > count; test \"$(cat count)\" -eq 10000; test \"$(head -n1 even)\" -eq 20000; test \"$(tail -n1 even)\" -eq 2",
    'hash': 'dd if=/dev/zero of=payload bs=65536 count=64 status=none; sha256sum payload > sums; sha256sum -c sums; test "$(wc -c < payload)" -eq 4194304',
    'links': 'printf content > original; ln original hard; ln -s original soft; cmp hard soft; test "$(stat -c %h original)" -eq 2; rm original; test "$(cat hard)" = content; test -L soft; test ! -e soft',
    'git': 'git init -q repo; cd repo; git config user.name Compatibility; git config user.email fixture@example.invalid; printf "one\\n" > file; git add file; git -c core.hooksPath=/dev/null commit -qm initial; test "$(git rev-list --count HEAD)" -eq 1; test -z "$(git status --porcelain)"; printf "two\\n" >> file; git diff --exit-code --quiet && exit 1; git diff --numstat | grep -q "1"; git restore file; test -z "$(git status --porcelain)"',
    'node': "node -e 'const fs=require(\"fs\"),crypto=require(\"crypto\"),assert=require(\"assert\");const data=Buffer.alloc(1048576,42);fs.writeFileSync(\"payload\",data);assert.deepStrictEqual(fs.readFileSync(\"payload\"),data);assert.strictEqual(crypto.createHash(\"sha256\").update(data).digest(\"hex\").length,64);setTimeout(()=>console.log(\"NODE_TIMER_PASS\"),10)' | grep -qx NODE_TIMER_PASS",
    'metadata': 'touch file; chmod 640 file; test "$(stat -c %a file)" = 640; truncate -s 1048576 file; test "$(stat -c %s file)" -eq 1048576; touch -d @1700000000 file; test "$(stat -c %Y file)" -eq 1700000000; test "$(basename /one/two)" = two; test "$(dirname /one/two)" = /one',
    'xargs': "seq 1 8 | xargs -n 1 -P 4 sh -c 'printf \"%s\\n\" \"$1\" > \"item-$1\"' sh; cat item-* | sort -n > actual; seq 1 8 > expected; cmp actual expected",
    'text-columns': "printf 'a:1\\nb:2\\nb:2\\n' > input; cut -d: -f1 input | uniq > letters; printf 'a\\nb\\n' > expected; cmp letters expected; tr 'a-z' 'A-Z' < letters > upper; paste -d: letters upper > pairs; printf 'a:A\\nb:B\\n' > expected; cmp pairs expected",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', action='append', required=True, metavar='LABEL=PATH')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--warmup', type=int, default=1)
    parser.add_argument('--timeout', type=float, default=600)
    parser.add_argument('--case', action='append', choices=CASES)
    parser.add_argument('--profile', action='store_true', help='diagnostic loader timings; measure separately from ordinary samples')
    args = parser.parse_args()
    if args.repeat < 1 or args.warmup < 0 or args.timeout <= 0:
        parser.error('repeat/timeout must be positive and warmup nonnegative')
    distributions = {}
    for entry in args.dist:
        label, separator, path = entry.partition('=')
        if not separator or not label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in label) or label in distributions:
            parser.error('use unique simple LABEL=PATH distributions')
        distributions[label] = Path(path).resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(scope=__doc__.strip(), root=str(args.root.resolve()), profile=args.profile,
                  distributions={}, results=[], summary={}, passed=False)
    destination = args.output / 'results.json'

    def save():
        destination.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')

    try:
        with ExitStack() as stack:
            pools = {}
            for label, dist in distributions.items():
                report['distributions'][label] = dict(path=str(dist), sha256=distribution_hashes(dist))
                pools[label] = stack.enter_context(InitPool(args.root, dist, args.output / label, size=2, timeout=args.timeout, profile=args.profile))
            for round_number in range(-args.warmup, args.repeat):
                cases = list(args.case or CASES)
                random.Random(round_number).shuffle(cases)
                labels = list(pools)
                if round_number % 2:
                    labels.reverse()
                for name in cases:
                    for label in labels:
                        identity = uuid.uuid4().hex
                        directory = '/tmp/daily-benchmark-' + identity
                        marker = 'DAILY_PASS_' + identity
                        script = ('set -euo pipefail; work=' + shlex.quote(directory) +
                                  '; mkdir "$work"; trap \'cd /; rm -rf -- "$work"\' EXIT; cd "$work"; ' +
                                  CASES[name] + '; printf "%s\\n" ' + shlex.quote(marker))
                        row = pools[label].run(['/bin/bash', '-c', script], expect=[marker])
                        row.update(distribution=label, case=name, round=round_number, warmup=round_number < 0)
                        report['results'].append(row)
                        save()
                        print(json.dumps({k: row[k] for k in ['distribution', 'case', 'round', 'status', 'request_to_exit_ms']}), flush=True)
            for label in pools:
                report['summary'][label] = {}
                for name in args.case or CASES:
                    rows = [r for r in report['results'] if r['distribution'] == label and r['case'] == name and not r['warmup']]
                    values = sorted(r['request_to_exit_ms'] for r in rows if r['status'] == 'passed')
                    report['summary'][label][name] = dict(passed=len(values), total=len(rows),
                        median_ms=statistics.median(values) if values else None,
                        p95_ms=values[math.ceil(len(values)*.95)-1] if values else None)
            report['passed'] = all(r['status'] == 'passed' for r in report['results'])
    except Exception as error:
        report['error'] = repr(error)
        raise
    finally:
        save()
    print(json.dumps(report['summary']), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
