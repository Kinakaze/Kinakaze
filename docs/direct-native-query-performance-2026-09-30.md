# Direct native queries and exclusive creation

The complete 357-package Node.js/npm installation has **not reached the
180-second target**. Short-operation results below do not establish an overall
installation speedup. All artifacts for this investigation are under
`artifacts/goal-install-180-20260930/`.

## Implementation

Named inode and verity EA queries can use an already pinned file handle when
its rights permit the operation. Insufficient rights retain the private-open
fallback. Queries still read live persistent state; they do not cache mode,
ownership or integrity metadata. A pending shared-handle query watches its own
I/O status block and cancels only its own request. A signal cannot cancel a
different read on the same file object, and the request retires before its
buffers leave scope.

Positioned ordinary reads can likewise use the retained asynchronous, seekable
data handle. Existing integrity enforcement and private-reader handling remain
in place. The integrated source also contains independently developed catalog,
descriptor metadata, private-reader and sparse-copy changes; the combined
results cannot isolate this implementation.

Exclusive native regular-file creation now goes directly to the atomic create,
without first opening the target to determine whether it is a directory. The
native `STATUS_FILE_IS_A_DIRECTORY` result is translated to `EEXIST` only for
exclusive non-directory creation. An existing directory is rejected without a
second pathname lookup. Ordinary directory opens retain their existing path.

Initial user-namespace ID translation uses the validated, immutable identity
mapping directly. It checks the current namespace on every call, retains the
unmapped all-ones sentinel and uses the ordinary mapping path after a namespace
transition. Initial setgroups policy is also immutable; noninitial namespaces
still read their current policy.

## Validation

The combined source passed 99 focused native tests and four guest probes
(`NativeReadCacheProbe`, `NativeLookupProbe`, `SparseForkStackProbe` and
`ForkProviderMetadataProbe`). The follow-up source passed **100 native tests**;
one pre-existing native capability diagnostic was explicitly ignored. The
follow-up report is `r2-fixed-tests.json`.

The corrected follow-up Release package also passed `NativePermissionProbe`
and `NamespaceServicesProbe`; results and distribution hashes are in
`r2-guest-probes/report.json`.

Coverage includes concurrent request cancellation, limited handle rights,
unchanged file offsets, inode identity after rename/unlink/fd reuse, later
verity enable and corruption, transaction recovery, exclusive-create errors
for all three access modes, and user-namespace unshare/map/restore transitions.
The initial directory error regression failed before its native status
conversion was corrected; that failure is retained in `r2-tests.json`.

Release builds and sources are recorded separately. The first integrated
source is preserved in `source-integrated-r1` and described by
`integrated-source.json`. The follow-up is preserved in `source-integrated-r2` with
`r2-source.json`. Packaged distributions remain separate from build outputs.

## Measurements and limits

Ten thousand checked one-byte positioned reads, alternating the fixed original
and first candidate distributions, took 630.5, 167.0, 168.1 and 593.8 ms
respectively. Each run checked returned data, EOF and the unchanged descriptor
offset. This is a short-read result, not an installation prediction; see
`read-path-comparison/report.json`.

| Full installation | Install time | Node/npm, audit and file verification |
| --- | ---: | --- |
| Original fixed build | 390.287 s | Passed |
| First direct-query candidate | 484.232 s | Passed |
| Combined build | 409.671 s | Passed |
| Follow-up with corrected recorder | 408.789 s | Passed |
| Metadata candidate, corrected libc entry | 323.576 s | Passed |

These runs do not establish an overall improvement. Other compilation
and guest workloads were active. In addition, the original host-load recorder
queried detailed information for every host process before filtering names;
the combined run exposed **136.8125 seconds of recorder CPU**. Its recorder was
replaced during the run. The replacement sampled the remaining 231 intervals
in 0.658 seconds of total sampling wall time. Consequently, the combined run
is functional validation, not a clean performance comparison. The earlier two
runs used the same costly recorder and are also confounded.

The combined seed additionally contains the debconf preconfiguration repair,
and its package work created 5,371 native processes versus 5,255 in the original
run. It is not an identical-work comparison with that original seed. The
absolute 180-second target continues to require the complete corrected work.

The three `install-*/report.json` files retain results and distribution hashes.
The combined run's authoritative log is `integrated-install.log`, with corrected
load samples in `install-integrated-competition-fixed.jsonl`. A later attempt
was rejected before running any guest command because the root was already
installed; it overwrote the original combined environment and initial load
sidecars. Those sidecars are not evidence for the completed run.

The next runner rejects an existing output or installed root before writing
anything. It filters process names before collecting details, records its own
CPU and sampling time, checks source/distribution hashes, and still requires
all 357 packages plus Node/npm, audit and file verification. It uses ordinary
maintainer scripts, triggers and durability operations.

The follow-up completed with **0.672 seconds of recorder CPU** and 0.822
seconds of sampling wall time. Its 204 installation samples all observed active
foreign workloads. The guest Job used 385.250 CPU seconds (139.688 user,
245.563 kernel), created 5,371 processes, and recorded 44,605,777 page faults.
File verification passed in 17.437 seconds. See `install-r2/report.json` and
`r2-environment.json`. Removing the recorder overhead did not make this run
materially faster; no percentage improvement is claimed.

Periodic sampling observed the main unpacking dpkg process at 122.375 CPU
seconds, including 102.875 seconds in kernel mode. This is a lower bound from
its last sample, not its final exit accounting. Only 258 process lifetimes were
observed during the installation window; most short-lived helpers were missed.
Further diagnosis therefore needs lifetime accounting, including creation and
exit, before attributing the remaining Job CPU to individual helpers.

## Complete process lifetime diagnosis

`full-process-census/report.json` records another complete fresh installation
with the unchanged r2 distribution. The 357 packages, Node/npm, empty audit
and file verification passed. Installation took **343.274 seconds**, with
313.047 seconds of Job CPU. This was a diagnostic run, not a new candidate or
an isolated speed comparison. The different result from the same binary also
shows why concurrent-host timings cannot establish an optimization's benefit.

An IO completion port attached before the owned init process was resumed
captured 5,408 native process lifetimes. The installation ancestry contains
exactly 5,371 processes, matching the installation Job count. Process handles
pin identities until final CPU counters can be collected; an early Job exit
notification is not mistaken for a signaled process. No accounting errors or
unfinished process records were reported. The observer and local repository
server together consumed 2.469 CPU seconds over the complete session.

| Installation process group | Count | Whole-lifetime CPU |
| --- | ---: | ---: |
| Exec into dpkg | 86 | 104.625 s |
| Fork from dpkg-deb | 1,785 | 48.625 s |
| Fork from dpkg | 1,443 | 47.969 s |
| Exec into dpkg-deb | 714 | 43.781 s |
| Exec into tar | 357 | 14.906 s |
| Exec into dpkg-split | 357 | 10.422 s |
| Exec into rm | 357 | 9.922 s |

Ancestry classifies complete native lifetimes. It does not split startup from
useful guest execution, and it excludes the init process's CPU from those
groups. Init used 18.672 CPU seconds across the whole session. The sums must
not be subtracted from elapsed installation time as predicted savings.
Detailed identities, counters and classification are retained in
`full-process-census/processes.json` and `classified.json`.

Two additional native experiments did not justify production changes. Corrected
PE import hints were tested only in copied diagnostic images; their alternating
process-loop timings varied substantially and did not isolate a benefit.
Retaining a native mutex saved about 37 ms per 10,000 uncontended transactions,
too small to prioritize over the measured process and filesystem costs. The
linker output and inode locking implementation were kept intact.

## Operation-scoped metadata candidate

The next candidate opens an eligible native namespace path once for chmod,
chown or timestamp mutation. The owned inode handle survives link detection
and the mutation, and a mount writer remains held for that operation. Paths
with links, dot components, attachments or overlay state keep the ordinary
resolver. No mutable inode metadata or pathname observation is cached between
operations. `KINAKAZE_NATIVE_METADATA=0` selects the original path in the same
binary for paired measurements.

Three new native tests cover retained inode identity after replacement and
unlink, hosted-link fallback, and namespace prefix/name escaping. The frozen
r3 source passed 160 distinct focused native tests; three existing diagnostic
tests remain ignored. Guest tests additionally exercise path mutation, hard
links, relative paths, chroot, and read-only bind mount policy. The new path
probe, native permission probe and namespace service probe all passed with the
optimization both disabled and enabled in `r3-fixed-dist`.

The initial guest probe exposed an existing chmod error: a trailing slash on
a regular file was accepted. The VFS now reports `ENOTDIR` before changing the
file, and libc chmod/fchmodat use that VFS path. This also connects the ordinary
libc entry to the new optimization. The existing chmod native test checks both
entry points. The initial failed build and guest reports are retained separately
from the corrected source, distribution and results.

`r3-metadata-comparison/report.json` records a disabled/enabled/enabled/disabled
comparison using one immutable binary. Each row below contains 3,000 mutations
and checks the final mode, owner, time and content.

| Operation | Disabled, first | Enabled, first | Enabled, second | Disabled, second |
| --- | ---: | ---: | ---: | ---: |
| chmod | 625.1 ms | 477.9 ms | 515.7 ms | 619.3 ms |
| chown | 588.1 ms | 483.6 ms | 485.3 ms | 611.5 ms |
| utime | 622.9 ms | 515.6 ms | 579.5 ms | 614.1 ms |

The corresponding 512-file dpkg unpack checks took 2.238, 2.159, 2.738 and
2.249 seconds, with ordinary durability operations and checked maintainer
scripts. These short dpkg results do not establish an installation gain.
Foreign workloads were active in all four intervals. A separate native
absolute/relative-open experiment also did not justify retaining another root
handle; its results remain diagnostic artifacts only.

`r3-startup-diagnostic-fixed/summary.json` splits 72 forks from a small, checked
dpkg fixture. Median native creation was 6.307 ms and startup readiness wait
was 14.600 ms, against a 24.578 ms median total fork time. Arena and other
mapping copies had medians of 0.894 and 1.789 ms. These are instrumented small
sample timings, not full-install savings. Nested phase totals must not be
added. Concurrent stderr interleaves exec tracing, so no exec-phase aggregate
is accepted from that stream; the per-process profile files are retained.

The corrected metadata distribution completed the full 357-package install in
**323.576 seconds**. Node/npm, empty dpkg audit and file verification all passed;
verification took 16.813 seconds. The installation Job used 308.766 CPU seconds
(110.438 user and 198.328 kernel), created 5,371 processes and recorded
44,628,585 page faults. The host recorder used 0.313 CPU seconds over the full
session. Concurrent foreign work remained active, and this integrated build
contains changes beyond the gated metadata path, so its difference from an
earlier run is not a causal measurement of this one patch. The 180-second
target remains unmet. Evidence is `install-r3/report.json`,
`r3-fixed-dist.json`, `r3-source.json` and `r3-environment.json`.

## Native fork prewarming integration

The r4 snapshot integrates the available portable fork transport and native
prewarming pool. It passed 309 distinct native checks, two manager integration
checks and 24 guest checks across ordinary, transferred and pooled fork modes.
Six existing diagnostic tests remained ignored. Traced semantic probes observed
39 pool hits; performance measurements disabled those traces.

A six-run ordinary/transfer/pooled/pooled/transfer/ordinary comparison used one
immutable distribution. The 128-fork loop took 5.372, 5.988, 3.337, 3.565,
6.608 and 4.911 seconds. The pooled loop used more Job CPU while reducing its
elapsed time. Later ordinary command-loop and dpkg measurements caught up with
the pooled results, so those small fixtures do not establish an end-to-end gain.

The complete fresh 357-package installation with both fork flags enabled took
**292.530 seconds**. Node/npm, empty dpkg audit and all file checks passed;
verification took 19.640 seconds. Installation Job CPU was 345.469 seconds
(119.391 user and 226.078 kernel), with 5,391 processes and 43,159,005 page faults.
The recorder consumed 0.344 CPU seconds. Foreign workloads were active, so this
result and r3 are separate integrated observations, not an isolated comparison.
The 180-second target remains unmet. Evidence is `install-r4/report.json`,
`r4-dist.json`, `r4-source.json`, and `r4-environment.json`.

Before preparing the standalone metadata commit, review found that its first
native open could traverse a Windows junction in an ancestor. The final helper
uses a native open with OBJ_DONT_REPARSE and adds a real junction regression
test. The r3/r4 results above precede that correction and remain preserved;
they are not acceptance results for the corrected binary.

## Corrected combined build

The metadata correction passed 30 focused native Release tests on its isolated
change. It is committed as `f2ee7bd`. The pool lock/refill change passed six
native lifecycle tests and 24 guest checks, and its four alternating local
comparisons are recorded in `native-fork-pool-locking-2026-09-30.md`. That change
is committed as `198838d`. Both commits were pushed to main.

The r6 distribution combines those corrections with the fixed integrated
source. All 24 guest checks passed again. Its first fresh complete installation
took **313.328 seconds**; all 357 packages, Node/npm, empty audit and file
verification passed. Verification took 15.316 seconds. Installation Job CPU was
379.344 seconds (130.031 user and 249.313 kernel), with 5,392 processes and
43,155,353 page faults. The observer used 0.547 CPU seconds. All 156 installation
samples observed active foreign work, including compilation, so this is not an
isolated comparison against r4.

Evidence is `install-r6/report.json`, `r6-source.json`, `r6-dist.json` and
`r6-environment.json`. The immutable r5 source is `source-integrated-r5`; the
current r6 source is `source-integrated`. A same-distribution follow-up uses
a fresh root and requires 20 consecutive seconds without observed competing
compiler/worker CPU activity before starting any guest command. Waiting is
outside all measured installation phases.

That same-build follow-up completed in **294.490 seconds**, with all 357
packages and every post-install check passing. Verification took 16.576 seconds.
Job CPU was 348.313 seconds, with 5,391 processes and 43,149,881 page faults.
The recorder used 0.484 CPU seconds. It started after the quiet prerequisite,
but foreign work resumed in 15 of 146 installation samples. This improves the
measurement conditions without establishing an idle-host result. The remaining
gap to 180 seconds is about 114 seconds; waiting for less competition alone
did not achieve the target. Evidence is `install-r6-quiet/report.json` and
`r6-quiet-environment.json`, using the unchanged `r6-dist.json` distribution.
