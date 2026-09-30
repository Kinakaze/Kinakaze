# Cumulative write-watch hints for private fork destinations

The guest's fresh zero-filled native stack and the coordinator's fresh private
fork destinations can skip unchanged pages by querying cumulative Windows
write-watch history. This removes an 8 MiB source scan from eligible forks and
reduces remote copies without inferring zero bytes from a stack pointer or a
working-set observation.

Provenance remains process-local and outside the mapping ABI. History is never
reset. The child adopts its own destination history only after all allocator
and TLS participants have finished restoring; parent WriteProcessMemory output
and later child CPU/kernel writes remain visible to subsequent forks. Mapping
replacement invalidates overlapping hints. Queries validate page granularity,
address bounds and counts, and scratch allocation is bounded to mappings of at
most 64 MiB. Failed optional allocation/query, unwatched memory and page-granular
section fragments retain the ordinary copier. Commit state and protection runs
remain page granular.

`KINAKAZE_FORK_WRITE_WATCH=0` disables all copy hints.
`KINAKAZE_FORK_WRITE_WATCH_CHILDREN=0` disables destination tracking and adoption.
Both are enabled by default. The flags and address scratch are prepared before
the allocator and sibling threads are frozen. Optional restore timing uses
static storage and a separate function frame, avoiding a premature 4 KiB stack
allocation before native TLS and allocator restoration.

The native tests cover CPU writes, kernel read output, remote writes, protected
pages, repeated cumulative queries, mapping replacement, unwatched fallback and
descriptor lifetime. The guest fixture checks sparse writes below the active
stack pointer, kernel pipe output, repeated parent forks, descendant forks and
independent child changes.

The integrated frozen distribution passed 113 focused native tests (one
existing ignored diagnostic) and seven guest probes. Three alternating
same-binary comparisons of eight chains of sixteen forks passed every exit
status. With descendant tracking disabled/enabled, their medians were:

| Measurement | Disabled | Enabled |
| --- | ---: | ---: |
| 128 chained forks | 5,744.224 ms | 5,222.488 ms |
| Whole-job CPU | 6,359.375 ms | 5,671.875 ms |
| Page faults | 1,626,563 | 1,362,060 |

Each run created 129 native processes. Ordinary repeated-parent fork medians
were 5,256.785/5,228.943 ms. Diagnostic comparisons established that three nested
forks changed from zero skipped bytes to 8,404,992 bytes each, but diagnostic
timings were slower with the extension enabled. These observations support the
descendant workload result rather than a general VFS speedup.

An independent fresh-root installation on that integrated distribution installed
357 packages in 307.245 s, with Node/npm smoke checks, empty dpkg audit and empty
complete file verification. It used 287.578 s job CPU, 5,255 processes and
41,562,068 page faults. The best validated installation from this chat remains
302.849 s. The 180-second target is unachieved. Other VFS/loader changes are
present in the measured integrated binary; the isolated commit is validated
separately so these times do not assert exclusive change attribution.

The isolated commit graph also built all 29 native distribution modules and
passed 30 focused kernel tests with no ignored tests. Its five packaged probes
passed: NativeLookupProbe, ConcurrentVforkProbe, SparseForkStackProbe,
WriteWatchStackProbe and AllocationGrowthProbe. This separately checks that the
committed implementation does not depend on other uncommitted workspace changes.

Reproduce the same-binary descendant comparison after building a distribution
and preparing an installed Debian root:

```powershell
python tools/benchmark-fork-write-watch.py --root <installed-root> --dist <distribution> --output <new-output-directory> --rounds 3
```

Evidence retained in `artifacts/apt-180-20260930/`:

- `r8-descendant-watch-sources.zip` and matching SHA-256 manifest.
- `native-r8-descendant-watch.json` and `guest-r8-descendant-watch/results.json`.
- `nested-watch-paired-r8/report.json` and `profile-r8-descendant-watch/report.json`.
- `node-r8-descendant-watch/report.json` and `node-r8-host.jsonl`.
- `remote-write-watch-probe.json`, covering three remote allocation forms.
- `native-commit-watch.json` and `guest-commit-watch/results.json`.
