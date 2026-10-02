# Cached Node installation: asynchronous writeback

The continued performance task retains the previous complete Node.js/npm
installation contract: 357 newly installed Debian packages, maintainer scripts,
triggers, normal fsync barriers, Node/npm smoke tests, empty dpkg audit and full
file verification. Package download is measured separately. The recorded R0
baseline is 230.415 seconds; the 100-second target is not yet established.

## Recovered implementation defects

The interrupted R1 implementation compiled, but its tests did not compile and
the asynchronous path silently fell back for dpkg's write-only handles. The
native `GetFileInformationByHandle` identity query returned access denied on
these capabilities. The R1 core-js diagnostic retained all 12,500 synchronous
data flushes despite 6,250 attempted asynchronous submissions.

The queue now obtains inode and volume identity with native information
queries that work without read-attribute access. It retains the original
capability and never reopens a path to gain data-write permission. A regression
constructs a restricted write handle, confirms the original query fails, and
checks that the replacement matches the full-access handle's identity.

Shutdown now closes queue admission and waits for accepted operations before
publishing the session stopping state. This is explicit because returning from
init can terminate handler threads without running the queue's destructor.
Pending errors remain reportable; shutdown reports an unconsumed error.

WAIT-only range requests retire preceding work without initiating another data
flush. WRITE-only requests use the bounded two-worker session queue, falling
back synchronously when the queue cannot accept them. Ordinary fsync still
waits and executes its full native durability barrier.

## Validation and evidence

- Seven native queue tests pass, including restricted rights, inode lifetime,
  bounded admission, completion ordering, shutdown and error reporting.
- Two VFS descriptor/writeback tests pass using their matching native DLLs.
- `AsyncWritebackProbe.py` checks rename, unlink, readonly attributes, offsets,
  invalid flags/descriptors, pipes and completion after the submitter exits.
  It passes with asynchronous writeback enabled and disabled.
- The actual guest probe produces `file-writeback-data` startup records and no
  identity-query rejection after the fix.
- `KINAKAZE_WRITEBACK_TRACE=1` reports up to 16 rejected identity requests.
  I/O trace submission/wait records now include acceptance or errno results.

R0/R1 evidence, the recovered source, immutable distribution hashes, R2 build
inputs and installation results are under `artifacts/apt100/`. The R2 build
uses the frozen R1 cohort plus the writeback fixes, avoiding unrelated concurrent
workspace changes. R1b/R1c/R1d are explicitly diagnostic init-only builds.

Diagnostic elapsed totals include nested calls, concurrent work and logging.
They are not additive predictions of wall-time savings. Host process timelines
remain part of each full-install measurement; foreign builds can still affect
the shared machine even when this task runs no compilation during installation.

## R2 full installation comparison

Both R2 runs use identical distribution hashes and installed package versions.
They install 357 new packages; the final status database includes the seed's
preinstalled debconf package as well. All eight phases pass, including empty
audit and full verification output.

| Async writeback | Install | Verify | Job CPU | Native processes |
| --- | ---: | ---: | ---: | ---: |
| Enabled | 264.835 s | 16.751 s | 341.016 s | 5,391 |
| Disabled | 306.871 s | 18.668 s | 391.438 s | 5,391 |

These are not clean causal speed measurements. The enabled run has external
compilers in 208/261 install-window host samples; the disabled run has them in
266/302 samples. Observed external CPU increases total 316.703 and 774.031
seconds respectively (sampling misses short-lived processes). The matched
artifacts and comparison are in `artifacts/apt100/r2-comparison.json`.

## Next candidate: native library reuse

Isolated init fork tracing (`KINAKAZE_NATIVE_FORK_TRACE=1`) records 99 hits and
one initial empty-pool miss in 100 fork/exec iterations. It avoids the global
fork trace's per-operation logging. This is a focused probe, not evidence about
every fork in a full installation, and does not justify increasing pool size.

R3 reuses a loaded module's owning Windows loader reference and loads catalog
images through their retained file pins. Shared and provider libraries no
longer reopen each path merely to obtain its canonical identity. Normal DLL
initialization, per-child provider initialization, registration and release
remain in place. `KINAKAZE_NATIVE_LIBRARY_REUSE=0` retains a same-build comparison
path through ordinary canonicalization and LoadLibraryExW.

Host and bridge tests pass (38 tests; one intentional helper ignored), including
same-named DLLs in different directories, retained references after an original
owner drops, direct file pins, transferred catalog pins and invalid pin indices.
The focused DLL tests also pass with reuse disabled. The frozen R3 distribution
builds successfully and passes the real guest asynchronous writeback regression.

An alternating off/on/on/off comparison in R3 runs 100 fork/exec iterations per
session with startup profiling. Median request-to-exit time is 3.516 s disabled
and 3.399 s enabled. Shared-library loading across 203 startup records falls
from 155.332 ms to 67.469 ms (about 57%); overall elapsed improves about 3.3%.
The profiler adds overhead and these are not full-install acceptance results.
Raw phase records and host samples are in `artifacts/apt100/r3-library-profile/`.

R3 full installation completes in **254.156 s**, with 357 new packages and all
checks passing; full file verification takes 13.119 s. Job CPU is 328.000 s
(108.297 user, 219.703 kernel), with 5,391 native processes. Download is a separate
1.934 s. The installed package versions match R2. External compilers appear in
118/251 install-window samples, so the 10.678 s difference from R2 enabled is
not a controlled measurement of this change alone. The original R0 score of
230.415 s is still lower. The 100-second goal remains open.

## R3 phase diagnosis and heap snapshot refresh

The complete R3 phase run passes all checks in 307.090 s with startup and fork
timings enabled, without per-operation I/O tracing. The native fork pool records
3,392 hits and 20 misses across 3,412 forks. Increasing its capacity is not
supported by these results. Runtime loading totals 31.778 s, provider setup
21.761 s, and fork mapping copying 22.346 s. These overlapping diagnostic sums
must not be added as predicted installation savings.

The main dpkg process forks 1,434 times and copies 18.720 GB, spending 17.405 s
in mapping copying. Its anonymous COW backing remains at the first fork, so
pages made private afterward are recopied at every later fork. R4 periodically
refreshes whole section-backed C-heap chunks to a new immutable COW generation.
Existing children retain the old section; future handle duplication uses the
new parent section. The refresh holds allocator locks and freezes sibling
threads, preserves protection runs, and restores the previous mapping and
current bytes if replacement fails. It runs every 16 forks and can be disabled
with `KINAKAZE_FORK_HEAP_REFRESH=0` for a same-build comparison.

Three native tests pass with matching test DLLs: old/new generation isolation,
readonly/noaccess protection preservation, real failed-map rollback followed
by successful retry, and rejection of partial mappings or the active stack.
The frozen R4 build succeeds. `ForkHeapRefreshProbe.py` holds an older child
across 65 parent forks, dirties a 12 MiB malloc allocation, checks fresh children
against each generation, and exercises readonly/noaccess data plus nested forks.
It passes with refresh disabled and enabled, and with the older fork transport.

An alternating off/on/on/off comparison reduces request-to-exit median from
2.399 s to 1.968 s (about 18%). Per run, child copying falls from 1,017,966,592
bytes to 354,029,568 bytes, with an additional 62,914,560 bytes refreshed in the
parent. Refresh itself takes 39.690–41.814 ms. This focused diagnostic does not
establish the full-install target. Evidence, hashes and host samples are in
`artifacts/apt100/r4-heap-profile2/`; the older transport result is in
`artifacts/apt100/r4-heap-legacy/`. The first comparison's behavioral checks
passed, but its harness cleared the fork timing variables; it is not used for
the copying claim. The corrected harness injects those variables after clearing.

R4's full installation completes in **241.076 s**, installing the same 357 new
packages and passing Node/npm, empty dpkg audit, and full file verification.
Download takes a separate 1.845 s; verification takes 11.465 s. Job CPU is
313.094 s (111.297 user, 201.797 kernel), with 5,391 native processes and
34,832,890 page faults. There are no external compiler samples in its 238
install-window host samples, but other guest sessions appear in 166 samples.
The 13.080 s difference from R3 is not a controlled causal speedup. R0 remains
the lowest raw full-install score, and the 100-second goal remains unmet.
`artifacts/apt100/r4-full-on2/` contains the accepted run. Its first attempt
fails during rootfs preparation with Windows access denied, before any install
timing; a fresh attempt with identical distribution and seed succeeds.

R4 phase diagnostics also pass the complete installation and checks, in
268.503 s. Main dpkg copying falls from R3's 18.720 GB to 13.301 GB; refreshing
adds 1.283 GB of parent copies and takes 0.932 s. Its mapping-copy time is
12.249 s, versus R3's 17.405 s. Across the session, mapping copying is 16.748 s
and fork transactions total 61.372 s. The corrected startup logs contain no
malformed records. The remaining copy volume warrants further work.

## R5 candidate: refreshable ELF images

A read-only working-set sample of the live dpkg process identifies 2,818,048
private resident bytes in one 2,957,312-byte ELF image, separate from its C
heap. Ordinary `MapViewOfFile` views reject placeholder-preserving unmap with
Win32 error 487. Creating the same 64 KiB and 2,957,312-byte views through an
exact placeholder supports replacement successfully.

R5 gives loader-owned ELF images a distinct `RefreshableImage` storage contract
and initially maps them over placeholders. Runtime may refresh these images
under the same freeze/rollback protocol as the C heap, before duplicating their
registered owner slots. Generic private file mmap views keep their existing
contract. Immutable ELF source snapshots retain their independent owners. Cold
pages in these immutable-backed images are faulted in before COW inspection,
so fresh clean pages need not be conservatively copied. Timing records separate
`heap_refreshed_bytes` and `image_refreshed_bytes` under `snapshot_refresh`.

Thirty-one native checks pass: six ELF mapping tests, nineteen fork mapping
tests, two anonymous COW tests, three snapshot-refresh tests and the handoff
contract check. They cover fixed addresses, source/old-view isolation, failed
map cleanup, protection preservation and ownership validation. R5's frozen
build succeeds. Both normal and older fork transports pass the real guest
generation/isolation probe, with nonzero ELF refresh bytes explicitly required.
The alternating same-build comparison reduces child copying from 997,326,848
to 261,103,616 bytes; it refreshes 62,914,560 heap bytes and 28,573,696 image
bytes. Request-to-exit medians are 1.827 s disabled and 1.576 s enabled (about
14%). These are focused diagnostic results, not the full-install score. Raw
records are in `artifacts/apt100/r5-snapshot-profile/` and
`artifacts/apt100/r5-snapshot-legacy/`.

R5 full installation passes all checks and installs the same 357 new packages
in **306.545 s**. Separate download and verification times are 2.171 s and
15.920 s. Job CPU is 346.516 s and page faults fall to 33,809,116, with 5,391
native processes. External compilers appear in 136/302 install-window samples
and other guest sessions in all 302. This run does not improve the full-install
score, and the changed host load prevents isolating a causal regression or
speedup. The target is still unmet. Evidence is in `artifacts/apt100/r5-full-on/`.

## Writeback RPC follow-up measurement

The R5 two-package core-js unpack diagnostic completes successfully in 36.248 s.
Its 893,334 records have no dropped records, parse errors, unresolved parents
or missing final checkpoints. There are 6,250 submit RPCs taking 0.747 s and
12,515 wait RPCs taking 0.794 s. In contrast, 6,265 actual `FlushFileBuffers`
calls take 8.936 s. An empty-queue fast path therefore has limited headroom in
this workload; no new shared-state/event mechanism has been implemented. These
nested/concurrent diagnostic durations are not full-install savings estimates.

The accepted diagnostic is `artifacts/apt100/r5-core-io3/`, with analysis in
`artifacts/apt100/r5-core-analysis/`. Two earlier attempts fail during rootfs
preparation with access denied and have no timed guest installation. Full-run
I/O context aggregates from existing R0 logs are additionally recorded in
`artifacts/apt100/r0-full-diag/native-context.json`; retain the raw process and
path correlation before proposing the next filesystem change.

The same core trace attributes 5.794 s to 6,254 `NtCreateFile` calls underneath
`fchown`, versus 0.647 s in the earlier synchronous R0 core trace. R5's code
unconditionally reopens the descriptor for metadata before checking ownership.
The same R5 distribution with asynchronous writeback disabled completes the
core diagnostic in 34.082 s. The identical 6,254 `fchown` reopens take only
0.715 s, while 6,250 synchronous data flushes take 5.816 s. Full fsync remains
8.855 s for 6,265 calls. This supports the hypothesis that asynchronous data
writeback moves much of its wait into the immediately following metadata
reopen, limiting overlap. Raw comparisons are in
`artifacts/apt100/r5-core-writeback-comparison.json`; both trace sets have no
dropped records or incomplete final checkpoints.

The next candidate is descriptor reuse for ownership metadata, verified first
with native pending-I/O tests and the same two-package trace. A future design
must retain permission checks, set-ID/capability clearing and request-specific
cancellation; simply treating unchanged UID/GID as a no-op would be incorrect.
New writable data opens currently omit metadata-read rights, and restricted
host ACLs must retain a working fallback if optional metadata rights are denied.
R6 now implements a locked unchanged-ownership check on a pinned native
descriptor, including permission and capability checks and the native mount
writer lease. Newly created writable files request optional metadata-read
rights with an access-denied fallback. Nineteen focused native checks pass.
The original direct-handle hook was extended to the ordinary descriptor path,
which otherwise reopens through the mount metadata pool first. The R6a release
build was stopped at the user's request to finish and push; no new core-js or
full-install performance result exists. The guest probe is prepared but not
yet run. See `docs/ownership-descriptor-reuse-2026-10-01.md` for the submitted
change's scope and validation limits.

R3 also exposes 31 interleaved startup records from init's writeback threads.
Startup logging now formats the complete record before one append write, and
the summary parser rejects malformed records explicitly. The original logs
remain intact. Raw evidence is in `artifacts/apt100/r3-phase-diag/`.

## R6 and R7: metadata overlap

The completed R6a distribution passes the ownership/writeback guest probes with
asynchronous flushing enabled and disabled. The guest capability-write assertion
was corrected to require the existing ABI's `EOPNOTSUPP`; native tests retain
unconditional coverage of clearing internally stored capabilities. R6a takes
28.810354 s for core-js, while a new R5 run on the same host takes 28.188028 s.
Removing ownership reopens alone does not demonstrate a speedup. All trace
records have complete final checkpoints, no drops and no parse errors.

Native overlap experiments show that 128-byte files stall even information
queries during data flushes, through either the same or separate native open.
With 4-MiB files, information queries proceed while EA updates and reopens still
wait. Reports are in `artifacts/apt100/native-metadata-flush4/` and
`artifacts/apt100/native-metadata-flush5/`. These observations distinguish the
small-file data/metadata interaction from handle acquisition overhead.

R7 adds a 2-ms eligibility window to queued data-writeback hints. A per-inode
explicit waiter expedites its requests; session shutdown expedites all queued
requests and drains actual native completion. Queue bounds, retained inode
handles, errors and the separate full fsync barrier are unchanged. The
`KINAKAZE_WRITEBACK_DELAY_MS` control permits same-build comparisons. Nine queue
tests pass, including immediate waiter/shutdown promotion of a 60-second test
deadline and retained completion errors. Both guest probes pass in both async
modes. The distribution differs from R6a only in `init.exe`.

Same-build core-js diagnostics:

| Metric | Zero delay | 2-ms delay |
| --- | ---: | ---: |
| Unpack wall time | 28.141532 s | 24.491394 s |
| Ownership elapsed | 5.362610 s | 0.305380 s |
| Actual data flushes | 6,250 | 6,250 |
| Data flush elapsed, concurrent | 5.422806 s | 4.966601 s |
| Full file flushes | 6,265 | 6,265 |
| Full file flush elapsed | 8.679512 s | 8.698672 s |

The approximately 13% diagnostic improvement is not the full-install score.
Evidence: `r7-delay-comparison.json`, `r7-core-delay2/`, `r7-core-delay0b/`, and
their `-analysis` directories under `artifacts/apt100/`. Both traces are complete
with no dropped records or parse errors. The first zero-delay setup attempt
fails before timing; its fresh-root retry uses identical artifacts. The first
full setup likewise fails before timing. Full acceptance is being measured in
`artifacts/apt100/r7-full-delay2b/` with all 357 requested packages and no tracing.
