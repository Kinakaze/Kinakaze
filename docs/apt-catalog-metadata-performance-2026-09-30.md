# Native catalog and descriptor metadata performance

The requested target is a fully validated Debian Node.js/npm installation in
less than 180 seconds. It has not been achieved. This document records measured
results and the semantic boundaries of the current changes.

## Shared native catalog

Each init pool now validates its native providers once and publishes a compact,
read-only catalog to its authorized workers, fork children and exec candidates.
The catalog retains source-file pins, including deferred exports. Each request
checks the native directory's names so additions require discovery again. A
mixed ELF directory, a malformed addition or an incompatible peer uses the
ordinary discovery path or reports its ordinary validation error. Remote handle
transfer rolls back if the reply cannot be delivered.

`KINAKAZE_NATIVE_CATALOG=0` selects ordinary discovery in the same distribution.
The alternating four-round comparison is retained in
`artifacts/node-install-20260930/catalog-paired/report.json`:

| 32-child shell workload | Ordinary | Shared |
| --- | ---: | ---: |
| Median wall time | 2,860.58 ms | 2,484.11 ms |
| Median Job CPU | 3,062.50 ms | 2,757.81 ms |
| Median page faults | 503,248 | 480,226.5 |
| Native process count | 65 | 65 |

This workload improved by 13.16% in wall time and 9.95% in CPU. It is not an
estimate of a full package installation. A separate startup-profile pair
confirmed that workers received the shared catalog. Profiling is excluded from
the timing comparison. Pool lifecycle validation passed all 15 cases; real dpkg
unpack/configure and a local apt transaction passed and checked all payloads.

The fresh full installation in `install-catalog/report.json` passed: all 357
packages were installed, Node.js 18.20.4 and npm 9.2.0 ran, and dpkg audit and
file verification were clean. Installation took **418.375 seconds**, with
403.703 seconds of Job CPU. The distribution hashes are included in that report.
This run did not overlap this chat's compilation, but other host work was
present. Its difference from earlier full-install runs is not a controlled
measurement of the catalog alone.

## Reused descriptor metadata opens

Native fstat and descriptor metadata changes can now check out independent
asynchronous metadata opens from the live open-file description. Dup aliases
share the owner, concurrent queries use different native file objects, and at
most two idle opens remain. Rights are reused; inode contents, ownership,
permissions and EOF are queried live. Overlay copy-up retains its ordinary
uncached opens.

An already validated writable data handle keeps fs-verity enable excluded by
native sharing rules. Fstat can therefore read native EOF and live inode
metadata without redoing the verity transaction. Imported handles retain the
ordinary validation path. Final close detaches all private caches under the fd
table guard and drops them after releasing that guard. Late queries retain
their original owner and cannot recreate a closed description. Fork/exec
rebuild private metadata opens lazily.

`KINAKAZE_FD_METADATA=0` disables this optimization for same-build comparisons.
`tools/benchmark-fd-metadata.py` alternates the switch and checks repeated live
metadata, aliases and unlinked inode data, plus an installation-style workload
with ordinary fsync. It records exact distribution hashes and host compiler
activity.

Native validation in `native-catalog-test.json` passed **138 tests**, with three
existing explicit diagnostics/benchmarks ignored. It covers private concurrent
opens, rights upgrades, replacement/unlink, alias lifetime, final resource
release, writeback, fs-verity, proc-fd reopening and mounted overlay behavior.
The six edited VFS source hashes were unchanged across that test run.

The three-round alternating comparison in `metadata-paired/report.json` passed
all twelve workloads on one fixed distribution:

| Operation | Ordinary | Reused |
| --- | ---: | ---: |
| 20,000 fstat calls, guest time | 1,221.70 ms | 157.61 ms |
| 4,000 chmod/chown/fstat groups, guest time | 1,058.91 ms | 256.69 ms |
| 512 files with metadata, fsync and unlink, total time | 2,599.04 ms | 2,269.80 ms |
| Same 512-file workload, Job CPU | 1,421.88 ms | 1,156.25 ms |

The installation-style workload improved by 12.67% in wall time and 18.68% in
CPU. Host samples recorded other chats' Rust compilation throughout the test;
the workloads alternate order but these results are not a quiet-host estimate.

Six real isolated dpkg/apt transactions, using 512 checked payload files and
eight maintainer-script children, also passed. The medians in
`metadata-dpkg-paired/report.json` are:

| Phase | Ordinary | Reused |
| --- | ---: | ---: |
| dpkg unpack | 3,445.41 ms | 2,609.99 ms |
| dpkg configure | 847.72 ms | 832.02 ms |
| apt installation | 5,564.03 ms | 4,967.10 ms |

The unpack median improved by 24.25%; the local apt fixture improved by 10.73%.
Each trial retains independent package state and checks every installed payload
and the maintainer-script marker. No durability policy was changed.

## Complete installation with descriptor reuse

`install-metadata/report.json` records a fresh complete installation on the
fixed `candidate-metadata` distribution. It **passed** all 357 package installs,
Node.js 18.20.4, npm 9.2.0, dpkg audit and file verification. The installed
package/version map equals the earlier catalog run. Installation took
**397.434 seconds**: unpack 378 seconds and configure 10 seconds (the dpkg log
has one-second resolution). Job CPU was 382.797 seconds, including 241.438
seconds in kernel mode. Native process count stayed at 5,255. Peak Job commit
was 767,856,640 bytes. **The 180-second target is still unmet.**

The earlier catalog distribution took 418.375 seconds, but the two distributions
include intervening workspace changes. A host-context monitor recorded other
compilers in 107 of 352 samples and a median total host CPU utilization of 49.9%.
This chat's compilation had finished before installation. The two full runs
are not an isolated comparison of descriptor reuse; its measured comparison is
the alternating same-build fixture above. Distribution hashes, phase boundaries
and Job metrics are retained in the report and `install-metadata-host-context.json`.

## Pending metadata completion

That installation's file verification took an unexpected 213.663 seconds with
only 27.125 seconds of Job CPU. The shared metadata completion path was updated
to use the file event as a wake hint, while retaining the particular request's
status as the only completion authority. A stale file event or an imported
handle without synchronization rights falls back to bounded polling; exact
request cancellation and buffer retirement remain intact.

The updated path passed 31 focused native tests, with one existing capability
diagnostic ignored, in `native-metadata-wait-tests.json`. The final fixed
`candidate-metadata-wait` distribution also passed `NativeLookupProbe`,
`NativeReadCacheProbe`, `ConcurrentVforkProbe` and `ForkProviderMetadataProbe`.
These check live native lookup, unlinked inode data, dup/fork/exec offsets,
concurrent process creation and native provider metadata after fork.

`metadata-verify-wait/report.json` repeats complete file verification on the
installed root. Both distributions passed with empty verification output:
the polling distribution took 25.657 seconds and the completion-event
distribution took 22.139 seconds. The original 213-second delay did not recur
on the control, so its entire difference cannot be attributed to the wait-path
change. These are single warm-root trials with intervening workspace changes,
not a controlled cold-I/O comparison.

## Final packaged complete installation

The fixed `candidate-metadata-wait` distribution was subsequently installed on
a new independent root made from the same original seed. The complete
transaction in `install-metadata-wait/report.json` **passed**. Its exact native
hashes were checked again after validation, and its installed package/version
map equals the previous metadata and catalog transactions.

| Final validation | Result |
| --- | ---: |
| Complete apt installation, excluding acquisition | **313.462 s** |
| Local acquisition | 2.303 s |
| dpkg unpack / configure, one-second log resolution | 298 s / 8 s |
| Installed real packages | 357 |
| Node.js 18.20.4 / npm 9.2.0 | Passed |
| dpkg audit | Passed, empty output |
| Complete file verification | Passed, empty output, 19.549 s |
| Install Job CPU | 297.813 s |
| User / kernel CPU | 108.563 s / 189.250 s |
| Native process count | 5,255 |
| Page faults | 43,144,643 |
| Other I/O operations | 12,392,250 |

The host monitor now filters process names before querying CPU counters. It
records its own CPU and sampling time: 2.000 CPU seconds and 2.122 seconds of
sampling wall time for the entire session. Installation contained 310 samples;
227 observed other compilers, with median host CPU utilization of 33.5%.
The monitor is retained as `run-metadata-wait-install.py`, and its observations
are in `install-metadata-wait-host-context.json`. This chat ran no compilation
or competing benchmark during the transaction.

This is a single final-build validation, not an isolated comparison of the
wait change. It is faster than the earlier 397.434-second transaction, but
intervening source changes and host contention prevent attributing that
difference to one optimization. **The 180-second target is still unmet.**
Normal package payloads, scripts, triggers and synchronization remain included.

Existing install-scoped process diagnostics in
`fork-provider-performance-2026-09-30.md` show substantial native process
creation and bootstrap work: 3,341 forks and 1,914 fresh exec starts. Their
phase durations are nested and overlap file work, so they cannot be added as
predicted savings. Moving native preparation off that path requires explicit
handle transfer and state restoration; the ordinary command pool alone does
not satisfy those requirements. No unvalidated pool substitution was included
in the final distribution.
