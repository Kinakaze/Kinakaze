# First-read inode mutex retention

The first read now acquires its ordinary inode transaction and transfers that
same handle into the private reader cache after releasing exactly one mutex
recursion. It avoids opening a separate cache mutex and waiting on it before
the first read. Subsequent reads still acquire the retained mutex and read live
verity metadata. A failed conversion keeps the ordinary acquisition path.

The native regression suite passed 55 tests, with one existing native mapping
diagnostic intentionally ignored. The new test verifies identical handle
ownership, nested acquisition, exclusion of another thread and release of only
the converting transaction's recursion. The suite also covers verity changes,
read-cache lifetime, concurrent reads, descriptor reuse and install metadata.

The release distribution passed all eight selected guest probes: NativeReadCache,
NativeLookup, ConcurrentVfork, WriteWatchStack, AllocationGrowth, ForkNativeTransfer,
ForkPrewarmPool and ForkNativeFallback. Native tests used the previously isolated
cache cohort; packaged guest checks used the newer committed main baseline.
Production packaging checked all 29 modules and 5,963 guest exports.

Five alternating comparisons used two frozen distributions with the same source
baseline, `6b70480`, differing only in the three first-read source files. Both
used `KINAKAZE_READ_MUTEX=1`. Each guest measured its own loop, excluding Python
startup and worker activation. Every read and account lookup checked its result.

| Loop | Control median | Candidate median | Change |
| --- | ---: | ---: | ---: |
| 10,000 open/read/close operations | 2,683.019 ms | 2,830.463 ms | +5.5% |
| 10,000 reads on one open descriptor | 52.789 ms | 56.273 ms | +6.6% |
| 20,000 account lookups | 2,217.695 ms | 2,233.189 ms | +0.7% |

These results do not demonstrate a speed improvement. Adjacent runs varied
substantially, and no host-contention timeline was collected for this comparison.
The change removes first-read setup work, but its wall-time benefit remains
unverified. This microbenchmark does not establish complete-install performance;
the 180-second installation goal remains open.

The [measurement JSON](measurements/vfs-first-read-2026-10-01.json) preserves
all ten rows, distribution hashes and SHA-256 references to the source archive,
native suite and guest results. Full logs and frozen sources are retained under
`artifacts/apt-180-20260930/`.
