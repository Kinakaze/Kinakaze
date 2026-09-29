"""Reproducible C++ multi-translation-unit clean and incremental builds.

Clean means deleted build outputs, not cold OS caches. Retains source and logs
in a unique directory for diagnosis and baseline/candidate reuse.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--units', type=int, default=32)
parser.add_argument('--jobs', type=int, default=4)
parser.add_argument('--directory', type=Path)
parser.add_argument('--link-directory', type=Path, help='generated Kinakaze ELF import libraries')
args = parser.parse_args()
if not 2 <= args.units <= 256 or not 1 <= args.jobs <= 32:
    parser.error('units must be 2..256 and jobs 1..32')
compiler = shutil.which('clang++')
if not compiler or not shutil.which('make'):
    raise SystemExit('clang++ and make are required')
root = args.directory or Path(tempfile.mkdtemp(prefix='clang-build-', dir='/root'))
root.mkdir(parents=True, exist_ok=True)
report = dict(directory=str(root), units=args.units, jobs=args.jobs, compiler=compiler,
              link_directory=str(args.link_directory) if args.link_directory else None,
              version=subprocess.check_output([compiler, '--version'], text=True, timeout=120), builds=[], passed=False)
header = '''#pragma once
#include <array>
#include <numeric>
#include <vector>
#include <string>
#include <algorithm>
template<int N> long compute(int x) {
    std::array<long, N> values{};
    for (int j=0; j<N; ++j) values[j]=(x+j)*(j%7+1);
    return std::accumulate(values.begin(), values.end(), 0L);
}
'''
(root / 'shared.hpp').write_text(header)
for i in range(args.units):
    (root / f'unit{i}.cpp').write_text(f'#include "shared.hpp"\nlong f{i}(int x) {{ return compute<64>(x)+{i}; }}\n')
declarations = '\n'.join(f'long f{i}(int);' for i in range(args.units))
calls = '+'.join(f'f{i}(7)' for i in range(args.units))
(root / 'main.cpp').write_text('#include <cstdio>\n' + declarations + f'\nint main() {{ std::printf("%ld\\n", {calls}); }}\n')
objects = ' '.join(f'unit{i}.o' for i in range(args.units)) + ' main.o'
(root / 'Makefile').write_text('CXXFLAGS = -O2 -std=c++17 -MMD -MP\n'
    f'OBJECTS = {objects}\nall: app\napp: $(OBJECTS)\n\t$(CXX) $(OBJECTS) $(LDFLAGS) -o $@\n'
    '%.o: %.cpp\n\t$(CXX) $(CXXFLAGS) -c $< -o $@\n-include $(OBJECTS:.o=.d)\n')
expected = args.units * sum((7+j)*(j%7+1) for j in range(64)) + sum(range(args.units))


def clean():
    for name in [*root.glob('*.o'), *root.glob('*.d'), root / 'app']:
        name.unlink(missing_ok=True)


try:
    for stage in ['clean-first', 'clean-repeat', 'noop', 'one-source', 'shared-header']:
        if stage.startswith('clean'):
            clean()
        elif stage in ('one-source', 'shared-header'):
            source = root / ('unit0.cpp' if stage == 'one-source' else 'shared.hpp')
            with source.open('a') as stream:
                stream.write('\n// incremental rebuild\n')
            # Account for filesystems with coarse timestamp granularity.
            newest = max(path.stat().st_mtime_ns for path in root.glob('*.o'))
            time.sleep(max(0, (newest + 1_000_000_000 - time.time_ns()) / 1e9))
            stamp = time.time_ns()
            os.utime(source, ns=(stamp, stamp))
        started = time.monotonic()
        with (root / f'{stage}.log').open('wb') as log:
            command = ['make', f'-j{args.jobs}', f'CXX={compiler}']
            if args.link_directory:
                if any(c.isspace() for c in str(args.link_directory)):
                    raise ValueError('link-directory must not contain whitespace')
                command.append(f'LDFLAGS=-L{args.link_directory} -Wl,-rpath-link,{args.link_directory}')
            result = subprocess.run(command, cwd=root,
                                    stdout=log, stderr=subprocess.STDOUT, timeout=600)
        row = dict(stage=stage, elapsed_ms=(time.monotonic()-started)*1000, exit_code=result.returncode)
        report['builds'].append(row)
        assert result.returncode == 0, row
        row['compiled_units'] = sum(' -c ' in line for line in (root / f'{stage}.log').read_text().splitlines())
        expected_units = (0 if stage == 'noop' else 1 if stage == 'one-source'
                          else args.units if stage == 'shared-header' else args.units + 1)
        assert row['compiled_units'] == expected_units, row
        actual = subprocess.check_output([str(root / 'app')], text=True, timeout=20).strip()
        assert actual == str(expected), (actual, expected)
        row['output_validated'] = True
        print(json.dumps(row), flush=True)
    report['passed'] = True
finally:
    (root / 'results.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report), flush=True)
print('CLANG_BUILD_OK')
