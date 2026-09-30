# apt installation target: 180 seconds

The active target is a complete, validated installation of Debian `nodejs` and
`npm`, including their 357 dependencies, in less than 180 seconds. The target
has **not** been reached. Measurements below are intermediate results.

The benchmark uses `tools/benchmark-node-install.py`, a fresh independent root
installed from `artifacts/node-install-20260930/seed`, and the original cached
archives served by a loopback repository. Download and dependency planning are
reported separately. Installation includes normal package extraction, metadata,
fsync, maintainer scripts, configuration and triggers. Node's SHA-256/runtime
check, npm version, dpkg audit and file verification must also pass.

## First measured round

Reports, distribution hashes and logs are retained below
`artifacts/apt-180-20260930/`.

| Distribution | Full installation | Unpack | Configure | Validation |
| --- | ---: | ---: | ---: | --- |
| `candidate-hotpaths-test` baseline | 388.274 s | 375 s | 7 s | Passed |
| `candidate-r1` | 369.593 s | 355 s | 8 s | Passed |

The complete transaction improved by 4.81%. The dpkg subdivisions have
one-second resolution. These single full-install runs do not establish a stable
median. There were concurrent source changes to provider discovery, process
startup and native process liveness, and unrelated Rust builds on the host.
The candidate therefore measures the complete packaged workspace; its overall
improvement cannot be attributed solely to the changes described below.
Baseline file verification overlapped a subsequent compilation and is excluded
from performance comparison; its correctness result passed.

Three alternating control/candidate runs in `deep-r1/report.json` measured:

| Operation | Control median | Candidate median |
| --- | ---: | ---: |
| 20,000 missing stat/lstat calls | 373.19 ms | 314.35 ms |
| 20,000 existing stat/fstat calls | 1,571.22 ms | 1,287.85 ms |
| 2,000 unchanged chmod/chown pairs | 347.46 ms | 269.95 ms |
| 10,000 pwrite/pread pairs | 7,702.87 ms | 61.13 ms |
| 128 fork/wait pairs | 5,284.63 ms | 4,506.42 ms |

The read/write probe checks bytes and the unchanged shared descriptor offset.
The control's individual read/write runs ranged from 6.97 to 22.73 seconds;
host variation is substantial. This microbenchmark is not a prediction of
installation time.

Three alternating 512-file package trials, with 12 maintainer-script children,
are in `paired-small-r1/report.json`. Medians were 2,423.93 → 2,281.27 ms for
unpack, 1,336.15 → 990.63 ms for configure, and 5,171.98 → 5,089.68 ms for apt
installation. Each package trial uses independent dpkg/apt state and validates
all payload bytes. Normal durability operations remain enabled.

## Implemented paths

- Query inode and verity EAs together through a private asynchronous metadata
  handle; grow the stack buffer only for larger native records. Regular-file
  EOF and verity state are read under the same inode transaction.
- Avoid rewriting an unchanged inode EA. Permission checks, set-ID removal and
  capability removal still execute for repeated ownership updates.
- Use the existing checked data-writer capability for reads on writable opens.
  Its native write access excludes fs-verity enable throughout pin/dup/fork
  aliases; imported handles retain the ordinary validation path.
- Trim zero pages at sparse fork-copy run boundaries while coalescing short
  gaps to bound remote-memory requests. Fresh-zero destinations and full-copy
  requirements for retained mappings are unchanged.

First-round native validation passed 95 tests, with one existing native
capability diagnostic skipped. The EA query tests include missing/deleted
records, reversed requested names and 48-KiB values. Further coverage checks
fs-verity transaction recovery, corruption, native handle rights, inode identity,
permissions, xattrs, sparse copies and fork mapping protection.
`NativeLookupProbe`, `SparseForkStackProbe` and `AllocationGrowthProbe` also
passed on the packaged distribution.

## Second measured round

The next implementation reuses one idle private read capability per open file
description. Every read still acquires the cross-process inode lock and queries
the current persistent verity state. Only the immutable lock identity is reused
while its owned inode handle prevents file-ID reuse. Concurrent requests own
separate asynchronous handles; pending requests retire before reuse or signal
dispatch. Final close retires cached data handles outside the descriptor-table
write guard. Fork/exec rebuild process-private capabilities lazily.

Ordinary reads with no verity record now use native EOF directly under the lock;
they no longer query file size before every read. Native-directory stat avoids
the verity transaction, since native directories cannot acquire a hidden data
tail. Dedicated tests exercise later verity enable, corruption, concurrent
positioned reads, replacement/unlink, fd slot reuse and imported write-only
handles. `NativeReadCacheProbe` adds guest dup/fork/exec and shared-offset checks.

The fixed, independently packaged distribution is `candidate-r2-isolated-fixed`.
It was built from `source-checkpoint`, rather than the concurrently changing
workspace, with distinct production and native-test target directories. The
captured source manifest is `source-checkpoint.json`; subsequent changes to
the optional fork timing buffer and shared-query completion are described here.
Production distributions remain fixed after packaging, and the benchmarks
retain their complete native hashes.

Native validation passed 105 tests, with one existing diagnostic ignored.
`NativeReadCacheProbe`, `ConcurrentVforkProbe`, `NativeLookupProbe`,
`SparseForkStackProbe` and `AllocationGrowthProbe` passed. Fork timing storage
was moved from an unconditional 4-KiB stack array to allocation-free static
storage guarded by a mutex. The array had caused concurrent vfork children to
overflow before the early restoration callbacks completed. Both ordinary and
profiling-enabled concurrent vfork now pass (`guest-r2-fixed` and
`vfork-profile-fixed`).

Three alternating runs in `deep-r2-isolated/report.json` measured:

| Operation | Control median | Candidate median |
| --- | ---: | ---: |
| 20,000 missing stat/lstat calls | 414.51 ms | 376.76 ms |
| 20,000 existing stat/fstat calls | 1,580.97 ms | 1,092.94 ms |
| 2,000 unchanged chmod/chown pairs | 357.81 ms | 61.86 ms |
| 10,000 pwrite/pread pairs | 7,383.67 ms | 63.89 ms |
| 10,000 readonly pread calls | 549.11 ms | 88.43 ms |
| 128 fork/wait pairs | 5,586.14 ms | 5,506.49 ms |

The readonly probe also checks later writes/truncation, EOF and an unchanged
descriptor offset. These are whole-distribution comparisons; the concurrent
workspace changes include shared-handle EA queries and process/startup work.
Fork wall time changed little in this comparison, despite the I/O improvements.

The fresh full transaction in `node-r2-isolated/report.json` **passed all
correctness checks but took 436.928 s**: 417 s unpack and 10 s configure.
Node.js 18.20.4 and npm 9.2.0 ran, dpkg audit was empty, and file verification
was empty. The installation's Job CPU was 401.391 s across 5,255 processes.
This round did not overlap this chat's builds or other benchmarks, but other
independent installation/build workloads were present on the host. It does
not establish a controlled full-install speedup and remains above the target.

File verification exposed a separate regression: 192.529 s wall time versus
20.393 s in the first round, despite only 28.500 s of Job CPU. This is being
investigated, rather than counted as a successful performance result.

## Third measured round

Shared native metadata queries currently retire exactly their own
IO_STATUS_BLOCK and never cancel unrelated data I/O. However, their pending
path waited only for the interrupt event with a 1-ms timeout, even if the
metadata completed promptly. The next change allows the file event to wake
the waiter while still requiring its own status block to show completion.
A stale shared event or missing SYNCHRONIZE rights takes the bounded fallback;
there is no unbounded loop on an already-signalled file event. A regression
test checks that an earlier request's completion cannot retire a different
pending read. Native validation passed 106 tests, with one existing diagnostic
ignored. Packaged concurrent vfork, read-cache and native-lookup probes passed.
The frozen sources are retained in `r3-io-hints-sources.zip` with their SHA-256
manifest, in addition to the immutable packaged distribution.

Three alternating five-package verification trials passed, with medians of
5.087 s for the control and 5.316 s for the candidate. Two alternating complete
verification trials also passed (`verify-all-paired-r3/report.json`): control
22.059/36.614 s, candidate 28.285/31.008 s, medians 29.337/29.647 s. These runs
did not reproduce the 192.529-s delay and do not establish a speedup from the
completion hints. The earlier delay's cause remains unproven.

## Fourth measured round

`candidate-r4-stat` combines hosted-link detection with the final native stat
query. It opens the inode once, queries the live inode/verity metadata under
the existing transaction, and uses that operation's completed stat. A hosted
or native link still selects the full walker. Missing observations and ordinary
non-stat lookups retain their existing paths. The observation is discarded at
the end of the syscall; it is not a persistent pathname cache. Native ACL mode
projection retains READ_CONTROL and its metadata-only access fallback.

Three alternating same-root trials in `deep-r4-stat/report.json` passed all byte,
offset, metadata, EOF and process checks. The 20,000 stat/fstat median fell from
1,613.738 to 870.685 ms (46.05%). This microbenchmark does not predict the full
installation time. Unchanged paths varied too, and the fork median rose from
5,262.109 to 5,988.168 ms; no fork improvement is claimed for this round.
Native validation passed 107 tests with one existing diagnostic ignored.
Packaged `NativeLookupProbe`, `NativeReadCacheProbe` and `ConcurrentVforkProbe`
passed. The new native test verifies fresh permissions, ownership, size,
replacement inode identity and link-following across subsequent observations.
Frozen source files and their manifest are retained as `r4-stat-sources.*`.

## Fifth measured round

`candidate-r5-integrated` freezes the later workspace changes together with the
stat observation optimization. These include descriptor metadata capabilities,
deferred fork provider metadata, retained source-file path queries and syscall
bridge template reuse. The source checkpoint's 97 changed/new files are retained
in `r5-integrated-sources.*`; changes from other concurrent chats are measured
as an integrated distribution, rather than attributed to this chat alone.

The complete fresh-root transaction in `node-r5-integrated/report.json` passed
all checks and took **302.849 s**, including 290 s unpack and 7 s configure.
It started 5,256 native processes, used 287.922 s of Job CPU (105.766 s user,
182.156 s kernel), and incurred 43,158,393 page faults. Node/npm checks and
empty audit passed; empty file verification took 15.592 s. Download took
3.221 s separately. The target remains unachieved. Sparse host observations
in `node-r5-host.jsonl` did not observe other compiler/runtime workloads, but
they do not establish that the host had no other work between samples.

Native focused validation passed 107 tests with one existing diagnostic
ignored. The fixed package also passed native lookup/read cache, concurrent
vfork, deferred fork metadata, sparse fork stack and allocation growth probes.

## Sixth round under validation

The next candidate avoids a full fstat before root's native descriptor chmod;
it validates the pinned inode's link type inside the already-required EA update
transaction. Non-root ownership checks and overlay pre-copy-up checks retain
their ordinary path. A generation check prevents an intervening close/reuse
from applying the captured policy to a new descriptor.

Native inode pins now retain one same-access duplicate per native open file
description, so repeated read/write/metadata operations clone a private Arc
instead of issuing DuplicateHandle/CloseHandle on every operation. Overlay
copy-up retains per-operation duplicates. Pins hold the original inode through
final close and descriptor reuse; process-private pins are rebuilt after
fork/exec. `KINAKAZE_NATIVE_PIN=0` selects the ordinary pin path for comparison.
The new correctness tests cover link/O_PATH rejection, live ownership and mode,
slot generation, inode replacement, dup aliases and final native-resource
release. Native validation passed 109 tests with one existing diagnostic ignored;
all six packaged guest probes from round five passed. The archive
`r6-chmod-pin-fixed-sources.*` includes the corrected regression test's native
duplicate setup; its earlier archive had a test-only compile error.

Three alternating same-root trials in `deep-r6-chmod-pin/report.json` passed.
Metadata changed from 58.842 to 53.366 ms, read/write from 62.219 to 60.380 ms;
readonly reads and fork were slower in that cross-distribution comparison.
Three further alternating sessions on the *same* binary switch native pin
reuse off/on (`native-pin-paired-r6/report.json`). Their read/write medians were
63.658/56.841 ms and readonly medians 91.740/88.218 ms. Other unchanged probes
also varied; these samples do not establish a stable overall gain.

Six real isolated 512-file package transactions with 12 maintainer-script
children passed all payload/marker checks (`paired-small-r6/report.json`).
Control/candidate medians were 2,145.024/1,992.126 ms unpack,
1,057.396/893.215 ms configure and 4,014.299/4,217.829 ms apt installation.
The separate guest I/O fixture's 128 fchmod calls fell from a median
13.385 to 7.310 ms. The complete apt fixture regressed by 5.07% despite these
phase improvements; no full-install speedup is inferred.

The complete independent transaction in `node-r6-chmod-pin/report.json` passed
all installation, Node/npm, audit and file-verification checks. It took
**311.198 s** (298 s unpack, 7 s configure), with 293.844 s of Job CPU,
5,255 processes and 43,162,387 page faults. File verification took 15.191 s.
An unrelated complete install was observed concurrently in `node-r6-host.jsonl`.
This does not demonstrate an end-to-end improvement over round five's
302.849 s, which remains this chat's fastest validated full transaction.

## Next fork investigation

The initial guest stack is a fresh 8-MiB private VirtualAlloc reservation. Its
ordinary sparse copy scans the full committed range before skipping zeros.
`probe-write-watch.py` is an isolated native API experiment, not a shipped
optimization. An empty write-watch query took 0.029 ms with two incidental
process page faults; the subsequent full zero comparison took 2.194 ms and
2,051 faults. Demand-zero reads were reported conservatively, so written-page
tracking must not be assumed to exclude all read-only touches. Fresh-allocation
checks successfully tracked ReadFile output, WriteProcessMemory output and CPU
writes, with repeated queries retaining the same pages (`write-watch-probe.json`).

[Microsoft's GetWriteWatch contract](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-getwritewatch)
requires a MEM_WRITE_WATCH allocation and allows observation without resetting
the tracking state. Any implementation must explicitly retain the allocation's
fresh-zero provenance, avoid resetting history, preserve all modified pages
including data below RSP, and use ordinary copying when tracking is unavailable.
This diagnostic does not establish a full fork or installation speedup.

## Seventh round: explicit write-watch stack copying

The initial guest stack now requests MEM_WRITE_WATCH, with an ordinary
VirtualAlloc fallback. A dedicated unsafe registration API records fresh-zero
provenance outside the serialized mapping ABI. Ordinary overlapping mapping
registration or unregistration invalidates that hint. No history is reset.
Fork scratch is allocated before the allocator/thread freeze; the frozen copy
queries and validates cumulative page addresses, sorts them without allocation,
and copies consecutive dirty pages while preserving each protection run.
Unsupported/invalid history uses the existing full private-memory copier.
Fork children receive ordinary private destinations and no watch provenance;
their later forks therefore use the established ordinary copying contract.

The frozen sources are round six plus these changes, rather than the concurrently
evolving main worktree. `r7-write-watch-fixed-sources.*` records 103 changed/new
files. The first archive/build used a misplaced windows-sys constant import;
the fixed archive has the correct SystemServices import and explicit feature.
Native focused validation passed **112 tests**, with one existing diagnostic
ignored (`native-r7-write-watch-fixed.json`). All seven guest probes passed,
including a new sparse stack fixture with kernel pipe output, repeated parent
forks, nested child forks and independent child writes. Concurrent vfork and
the new stack fixture also passed with optional fork diagnostics enabled.

Three alternating same-binary off/on trials (`write-watch-paired-r7/report.json`)
passed all checks. The 128-fork medians were 4,518.019/4,582.776 ms; this normal
benchmark does not establish a stable fork speedup. Unchanged metadata and
account probes also varied with host activity. Separate diagnostic runs of the
same two guest fixtures recorded 54 forks per setting
(`profile-r7-write-watch/report.json`). With tracking enabled, the median
explicitly skipped range was **8,323,072 bytes**; it was zero with tracking
disabled. Diagnostic mapping-copy medians were 3,808/3,115 us and total-fork
medians 32,367.5/29,901.5 us. These prove the new path was exercised, but do not
replace normal end-to-end installation measurements.

Two independent fresh-root installations ran sequentially on the same fixed
binary with KINAKAZE_FORK_WRITE_WATCH=0/1. Both installed all 357 packages and
passed Node/npm, empty audit and empty file-verification checks
(`full-r7-write-watch-paired.json`). The package hashes are identical. Neither
installation disables ordinary fsync, scripts or triggers.

| Setting | Install | Unpack/configure | Job CPU | Page faults | File verification |
| --- | ---: | ---: | ---: | ---: | ---: |
| Disabled | 410.718 s | 392/12 s | 396.188 s | 43,163,061 | 18.651 s |
| Enabled | 306.653 s | 295/6 s | 288.438 s | 41,598,462 | 16.104 s |

The enabled run started 5,255 processes, used 103.875 s user/184.563 s kernel
CPU, and performed 553,935 reads, 352,577 writes and 12,314,515 other I/O
operations. Sparse host observations show much heavier compiler activity during
the disabled run, including thirteen concurrent rustc processes in one sample;
both runs overlapped unrelated work. The 104-second timing difference therefore
cannot be attributed entirely to the copier. The observed page-fault reduction
was 1,564,599 (3.62%). The enabled installation did not improve this chat's
best validated end-to-end time of **302.849 s**. The **180-second target remains
unachieved**.

An independent owned-child native experiment now checks a possible extension
to fork descendants (`probe-remote-write-watch.py`,
`remote-write-watch-probe.json`). On this host, VirtualAllocEx and VirtualAlloc2
created remote write-watched private allocations. VirtualAlloc2 also replaced
an exact placeholder with a watched private reservation, which was then
committed separately. For all three forms, the child initially observed no
dirty pages, observed the parent's WriteProcessMemory output on page 3, and
retained pages 3 and 11 after its own CPU write and repeated queries without
reset. This is API evidence only: no production child-allocation or restore
path has been changed yet. Any extension must retain explicit fresh-destination
provenance, validate cumulative history in the child, handle mixed/protected
mapping fragments and keep the ordinary allocation/copy fallback.

Round eight implemented this descendant extension in a frozen graph based on
round seven. Fresh remote private reservations request MEM_WRITE_WATCH, with
the established private allocation or page-granular section fallback on failure.
After all child participants have adopted TLS and allocator state, the child
validates its own cumulative history before registering a private copy hint.
Parent WriteProcessMemory output is included in that history; it is never reset
or inherited as a parent-side address list. Reservations above 64 MiB cannot
register hints, bounding scratch allocation. Mixed and protected runs retain
their exact commit/protection and copying contracts.

`r8-descendant-watch-sources.*` records the fixed source graph. Native validation
passed **113 tests**, with one existing diagnostic ignored; all seven guest
probes passed. The new native test covers copied destination bytes, a protected
page, later CPU writes, cumulative repeated queries and unwatched fallback.
The guest stack fixture includes repeated parent and nested descendant forks.

Three alternating same-binary descendant-chain comparisons each performed
eight chains of sixteen forks, checking every exit status. All six runs passed
(`nested-watch-paired-r8/report.json`):

| Median | Descendant hints disabled | Enabled | Reduction |
| --- | ---: | ---: | ---: |
| 128 chained forks | 5,744.224 ms | 5,222.488 ms | 9.08% |
| Whole-job CPU | 6,359.375 ms | 5,671.875 ms | 10.81% |
| Page faults | 1,626,563 | 1,362,060 | 16.26% |

Each run had 129 native processes. Ordinary repeated-parent fork medians were
5,256.785/5,228.943 ms, a much smaller difference; unchanged VFS microbenchmarks
varied in both directions. Separate diagnostic runs recorded 54 forks per
setting. Three nested forks changed from zero skipped bytes to 8,404,992 bytes
each. Those diagnostic runs were slower with the extension enabled, so their
timing does not establish a speedup.

The independent fresh-root installation passed all **357 packages**, Node/npm,
empty audit and complete empty file verification
(`node-r8-descendant-watch/report.json`). Install time was **307.245 s**,
with 296/6 s unpack/configure, 287.578 s job CPU, 5,255 processes and 41,562,068
page faults. Verification took 18.863 s. The starting sparse host observation
found no compilers or other runtimes; a later sample observed five. This result
does not improve this chat's best validated complete installation of 302.849 s.
The **180-second target remains active and unachieved**.
