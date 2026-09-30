# Fork provider restoration and startup measurements

The requested complete Node.js/npm installation target is below 180 seconds,
excluding downloads. **The target has not been reached.** The latest integrated
pool-enabled installation passed in 283.206 seconds; its same-binary control
took 384.502 seconds under heavier compiler contention. The comparison below
separates these observations from the focused fork benchmark evidence.

## Changes

- Fork bootstrap grows its module pathname buffer only when needed and takes
  an owning Windows loader reference to an already loaded DLL before attempting
  ordinary DLL loading. The expected module base is still checked.
- A fork child immediately retains each restored provider's native mapping and
  the registry's immutable source-file pins. It reconstructs symbol metadata
  only when symbol lookup or loader introspection requests it. Lookup failures
  remain errors, are retained, and are never treated as absent dependencies.
  `KINAKAZE_FORK_PROVIDER_METADATA=eager` restores eager metadata construction
  for a same-build comparison.
- Sorted symbol and version declarations use their ordering for duplicate
  validation and lookup. Unordered adapters retain the general lookup and
  validation paths. PE export grouping sorts by symbol name and version
  separately, preserving names such as `atan`, `atan2` and versioned `atan`.
- Deferred providers obtain canonical file identity through their retained
  source-file handles, including handles transferred with the shared catalog.
  This avoids reopening every DLL pathname. Transfer record order determines
  the pin index; sorting provider module IDs does not change that association.
- Capability queries whose requested PID is zero avoid an unnecessary current
  PID lookup. Explicit PIDs retain the existing identity check.

No package, maintainer script, trigger or durability operation is removed.
Native image identity, version validation, module lifetime and fork address
checks remain mandatory.

## Measurements so far

Artifacts are under `artifacts/apt-180-20260930/` unless otherwise specified.

The `root-p2-install` fresh-root transaction installed all 357 packages in
405.305 seconds. Node.js, npm, dpkg audit and file verification passed. Its
earlier baseline took 407.347 seconds. Both runs overlapped other host work;
these results do not establish a controlled end-to-end improvement. These
installation runs predate lazy fork provider restoration and retained-handle
canonical path queries.

The diagnostic fork facade-restoration median fell from 3,397 microseconds in
`root-p2-fork-profile` to 40 microseconds in `root-p3-fork-profile`. Each report
contains 48 fork restorations. This is one measured phase, not total fork time;
profiling itself writes logs and is excluded from performance comparisons.
The packaged P4 run measured 29 microseconds for that phase and 4,480.5
microseconds for registry restoration. Its complete profiled fork transaction
median was 40.420 milliseconds, including 6.621 milliseconds creating the
Windows process and 15.048 milliseconds waiting for bootstrap readiness.

Four unprofiled 128-iteration same-build sessions used the order eager, lazy,
lazy, eager. All C fork, fork/exec and posix_spawn cases passed. Host contention
was substantial: one eager fork/exec batch took 25.215 seconds versus 12.362
seconds for the other eager batch. The lazy batches took 12.527 and 11.455
seconds. These samples do not justify a stable percentage improvement.

`root-path-bench.json` compares 40 alternating batches of 35 DLL paths, checking
identical returned paths. Opening/querying/closing each pathname had a median
of 2.091 milliseconds; querying retained file handles took 0.735 milliseconds.
This isolated native measurement is not a full worker-startup measurement.

## Validation

`root-fork-loader-tests.json` records 77 passing release native tests covering
fork mappings and sparse copies, symbol versions and aliases, deferred errors,
source pin lifetime, transferred pin indices and canonical paths. The packaged
P3 candidate also passed `ForkProviderMetadataProbe`, `ConcurrentVforkProbe`,
`SparseForkStackProbe`, and late static TLS loading. The new provider probe
performs a nested fork before metadata lookup, then exercises versioned lookup,
dladdr, dl_iterate_phdr and reopening the inherited facade in all three
processes.

The packaged P4 candidate passed those three fork probes plus
`NativeReadCacheProbe`. Two private 2,048-file dpkg transactions passed and
validated every payload byte and their maintainer-script marker. Unpack took
12.441 and 9.573 seconds under concurrent host workloads; these are diagnostic
results, not a quiet-host performance estimate.

A further `node-core-js` verification comparison on one installed root used
P2, P4, P4, P2. The first run took 77.510 seconds; the subsequent runs took
1.284, 1.202 and 1.267 seconds. All passed. This large first-access effect also
affects interpreting complete verification timings; it is not evidence that
P4 alone improved verification by that ratio.

P4's fresh complete installation passed in **409.502 seconds**, with download
reported separately at 3.128 seconds. It installed all 357 packages; Node.js,
npm, dpkg audit and complete file verification passed. Verification took 29.102
seconds. The installation used 402.531 seconds of Job CPU and 5,255 native
processes. Its 12,420,109 other I/O operations and 43,140,162 page faults are
reported alongside the exact distribution hashes in
`artifacts/node-install-20260930/root-p4-install/report.json`.

This full transaction does **not** demonstrate an end-to-end speedup over the
407.347-second baseline or the 405.305-second P2 run. The host ran other
independent compilations and compatibility work. The runner records competing
tasks in `root-p4-install-competition.jsonl`; this is not an uncontended paired
comparison. The 180-second goal remains open. The remaining work is dominated
by native process creation/bootstrap, state restoration and synchronous file
operations; the improved provider phases alone cannot close the gap.

## Complete transaction diagnostics

A separate P4 diagnostic transaction passed with all 357 packages, normal
scripts/triggers/synchronization, Node/npm smoke checks, dpkg audit and file
verification. Installation took 461.135 seconds, download 3.034 seconds and
verification 22.793 seconds. Fork/startup/loader tracing and 294 native stack
snapshots were enabled. **This is not a performance acceptance run.** Its
installation wrote 3,645,584 I/O operations versus 352,577 in the unprofiled P4
run; profiling overhead cannot be ignored. Other host workloads also ran.

The phase timestamps in the report allow the analyzer to exclude update,
download, plan and post-install validation. The install itself contains exactly
3,341 fork transactions and 5,255 native worker starts, including 1,914 fresh
exec starts. The full diagnostic session has 3,354 fork transactions.

| Install-scoped diagnostic phase | Count | Sum | Median |
| --- | ---: | ---: | ---: |
| Complete fork transaction | 3,341 | 160.525 s | 46.792 ms |
| Native fork materialization | 3,341 | 115.091 s | 33.294 ms |
| Create native fork process | 3,341 | 28.953 s | 7.442 ms |
| Wait for fork bootstrap readiness | 3,341 | 57.537 s | 16.554 ms |
| Copy/install guest mappings | 3,341 | 21.493 s | 4.659 ms |
| Copy private managed arena | 3,341 | 3.603 s | 1.040 ms |
| Worker opens native runtime | 5,255 | 38.746 s | 7.070 ms |
| Provider discovery, sharing and binding | 5,255 | 23.773 s | 4.332 ms |

These rows are nested and can overlap between processes. They must not be
added together or subtracted from installation wall time as predicted savings.
In particular, opening the runtime is part of bootstrap readiness, and provider
restoration is part of the fork transaction. The 461.621-second `init-pool-wait`
span belongs to an unused standby worker waiting for activation, not a delay
on the installation's critical path.

Of 294 native snapshots, 163 stopped in `NtWaitForMultipleObjects`. Stack
candidates associate 108 with child-process waits and 26 with fork bootstrap;
28 have a VFS read-wait pattern. Export-nearest symbol names do not establish
the exact private VFS function. Other instruction pointers include 29 file
creates, 29 full/data flushes and 10 file-information writes. Sampling selects
the process with the greatest recent CPU increase, then inspects its busiest
cumulative thread; these counts are diagnostic observations, not an unbiased
wall-time percentage or a complete critical-path reconstruction.

The shared stderr trace contains interleaved writes from multiple processes.
The analyzer rejects malformed lines and computes a phase delta only between
adjacent recognized phases of the same PID. These incomplete stderr samples
must not be presented as exact transaction-wide totals. Per-process fork and
startup files provide the counts in the table above.

Artifacts:

- `artifacts/node-install-20260930/root-p4-install-profile/report.json`
- `artifacts/node-install-20260930/root-p4-install-profile/analysis.json`
- `artifacts/apt-180-20260930/root-profile-install.py`
- `artifacts/apt-180-20260930/root-analyze-install.py`

A separate native Rust experiment compared creating/joining a thread with a
one-shot Windows thread-pool wait on an already signaled event. Eight alternating
512-operation batches measured medians of 26.476 ms and 2.975 ms respectively.
The difference is only about 46 microseconds per operation in that fixture;
it does not justify treating init's exit-watcher thread creation as a principal
installation bottleneck. No production watcher implementation was changed.
The source and results are `root-exit-watch-bench.rs` and
`root-exit-watch-bench.jsonl` in the same artifact directory. The experiment
uses the documented one-shot registration and asynchronous close semantics of
[SetThreadpoolWait](https://learn.microsoft.com/en-us/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-setthreadpoolwait)
and [CloseThreadpoolWait](https://learn.microsoft.com/en-us/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-closethreadpoolwait).

The next substantial process optimization requires moving native preparation
off the fork/exec critical path or reducing native process replacement work.
The existing command pool does not serve these internal forks or execs.
Reusing its workers directly is incorrect: VFS launch frames currently contain
inherited raw handle values, and vfork also transfers three native event handles.
Any pool extension needs explicit capability transfer/remapping, source/target
identity checks, failure rollback and unsupported-state fallback before it can
preserve descriptor, namespace, signal and exec-activation semantics. No such
extension has been implemented or validated by this diagnostic run.

## Portable VFS capability transport prerequisite

The VFS now offers `serialize_portable_fork_state` and
`transfer_fork_state_handles` for a destination that does not inherit its
parent's native handle table. This is a prerequisite for preparing internal
workers in advance; the production fork coordinator still uses its ordinary
snapshot and process-creation path. No installation speedup is attributed to
this addition, and it does not satisfy the 180-second target by itself.

The optional frame section 31 records distinct source/destination HANDLE pairs.
Only explicitly identified handle fields contain indexes into that table;
Linux fd numbers, object identities, namespace IDs and socket recipe keys are
unchanged. The serializers cover the descriptor table, open-description pins,
native and overlay mount pins, FIFO markers, pipe-inode sections, Unix socket
state, Winsock transfer pipes and user-network backing pins. A restore scope
rejects an unindexed native handle in a portable frame, malformed indexes and
duplicate source/destination entries. Legacy snapshots retain their previous
representation. Portable standard streams become owned duplicates; native
console handles and IoRing descriptors reject this transport with `EOPNOTSUPP`.
Winsock continues to use `WSADuplicateSocket`, never generic handle duplication.

All native duplicates must succeed before the frame's destination fields are
updated. Failure closes only duplicates made by that invocation and preserves
the frame. Source columns remain available for a retry after the failed or
abandoned destination is terminated. The unsafe public transfer contract
requires stable source owners and an unpublished destination that restores
the frame once or terminates. This is not a general snapshot-lifetime owner or
a worker-recycling API.

Release compilation and 140 focused native tests passed, with three existing
diagnostic tests ignored. Seven tests belong to the new transport module,
including its subprocess fixture. The cross-process case creates independent
handle tables and restores a complete VFS fork frame. It verifies a file and
its dup alias share the child's advanced offset, anonymous-pipe and FIFO data,
Unix socket data, a shared eventfd counter, and an actual Winsock UDP endpoint
reconstructed with the same bound port. Other transport tests check nested
scope rejection, unwind/error cleanup, malformed frames, unsupported descriptor
kinds, retrying from original sources and handle-count stability after sixteen
partial failures. Expected caught-panic diagnostics in the log belong to the
unwind cleanup test. Existing fork/exec, OFD, FIFO, pipe, Unix socket, Winsock
and user-network regressions also passed.

Commands and evidence:

```text
cargo test --locked --release --lib -p kinakaze-vfs --no-run --message-format=json --target-dir target/apt-syscall
python artifacts/apt-180-20260930/root-portable-tests.py native_transfer::tests fork_handoff::tests pipe_inode::tests fifo::runtime::tests unix:: socket:: usernet:: ofd::tests tests::exec_ tests::mapped_exec_ tests::every_fd_kind_ tests::winsock_descriptors_
```

- `artifacts/apt-180-20260930/root-portable-build.jsonl`
- `artifacts/apt-180-20260930/root-portable-build.log`
- `artifacts/apt-180-20260930/root-portable-tests.json`
- `artifacts/apt-180-20260930/root-portable-regression.log`

These are native process tests, not a packaged guest fork test or a new full
installation measurement. Portable nonempty overlay/mount transfer still needs
dedicated cross-process coverage. Remaining integration includes the runtime
handoff callback, vfork's native events, explicit transfer contracts for other
fork participants, bounded init-owned worker preparation, reservation/parent
death rollback and an ordinary-path fallback for unsupported state. The
existing command pool initializes logical process state and cannot simply be
substituted for a native fork bootstrap worker.

## Runtime transport and bounded native fork preparation

The portable VFS format is now integrated with the fork coordinator behind
`KINAKAZE_FORK_TRANSFER=1`. Each participant must explicitly declare that its
complete state is independent of ambient handle inheritance, or provide a
portable snapshot/transfer callback. Unknown participants and legacy stage
hooks retain the ordinary path. Unsupported VFS descriptors also restage an
ordinary snapshot. Contracts survive nested forks and follow the selected
participant owner, including priority replacement. The coordinator transfers
participant capabilities and vfork events into a stopped, unpublished child
before arena copying. An address-space retry starts from original source
handles. Portable cold creation inherits only explicit bootstrap handles and
owned standard-stream duplicates.

`KINAKAZE_FORK_POOL=1` additionally enables init-owned native preparation in
sessions configured with a command pool. This is a distinct supply of at most
two unused native workers. It matches module paths/bases and the TLS slot mask,
pins modules in the configured native distribution, and gives workers no Linux
PID until fork adoption. Stock blocks on an activation event; it signals a
second ready event after returning to user mode, before the parent suspends it.
Cold creation remains available on a miss. Every used worker is consumed once.
Init retains a rollback owner until transaction commit, observes pinned parent
death, terminates candidates on abort or failed reply delivery, and covers
creation with its kill-on-close Job. Pending native deliveries are bounded at
128. These switches remain opt-in pending broader installation validation.

The transport's first packaged probe passed with a portable frame in both
parent/child generations. It checks shared file offsets, CLOEXEC through exec,
pipe/FIFO/Unix socket data, an eventfd counter, UDP identity, private/shared
mappings and signal masks. Seven packaged probes then passed with transport
both off and on. The initial native pool distribution subsequently passed eight
probes in both pool modes without diagnostics. The new repeated-fork probe
changes cwd, environment, file data and memory before every fork, includes
nested forks and exec, and checks that no earlier guest state is reused.
Diagnostic runs recorded 112 pool hits and one miss. Idle inventory showed
exactly two native stock workers and two ordinary command workers, with zero
Job CPU milliseconds over the one-second idle observation. Unix-rights keeper
helpers and their console hosts also occur in the pool-disabled control; they
are recorded separately in the process inventory.

Management/protocol tests passed 68 cases, including reservation ownership,
adoption/commit/abort and parent death. Three additional init native tests use
live subprocesses to verify abort/death termination, successful commit survival
and exact failed-delivery cancellation. The test fixture is included in that
three-test count. A diagnostic stress run exposed an existing lock inversion:
printing while the arena was frozen could wait for a sibling holding stderr
and waiting for allocation. Frozen-copy diagnostics now use a bounded,
host-private buffer, flushed after both freezes end, including error unwinding.
Twenty-seven focused kernel release tests passed, including a sibling-held
stderr regression and the existing transport/participant/mapping tests. The
fixed distribution passed eight packaged diagnostic probes, and the ordinary
path passed three consecutive concurrent-vfork diagnostic runs. A separate
fallback probe loaded an unaudited X11 participant: both nested forks selected
ordinary inheritance and preserved pipe data. Its original fixture incorrectly
asserted an unrelated XKeysym result before reaching fork; the corrected fixture
loads the participant and directly checks the intended fallback behavior.

Four unprofiled runs of the same hash-pinned pool distribution used the order
off/on/on/off, with portable transport enabled in both settings. All 128-iteration
C fork, fork/exec and posix_spawn batches passed:

| Batch median, milliseconds | Pool off | Pool on |
| --- | ---: | ---: |
| C fork + wait, 128 iterations | 4101.172 | 2324.908 |
| fork + exec + wait, 128 iterations | 8755.374 | 6707.316 |
| posix_spawn + wait, 128 iterations | 3965.662 | 3835.781 |

These are actual batch durations, not sums of nested phase timings. They do
not establish the complete installation result. The full comparison follows;
the 180-second target is still unmet. Portable nonempty overlay/mount transfer and other optional
participants require further coverage before enabling the optimization broadly.

New evidence under `artifacts/apt-180-20260930/`:

- `root-transfer-guest-on/`, `root-transfer-guest-off/`,
  `root-transfer-guest-on-regression/`
- `root-pool-manager-tests.log`, `root-pool-lifecycle-tests.log`,
  `root-pool-native-tests.json`
- `root-pool-identity2/` (diagnostic inventory),
  `root-pool-guest-normal-off/`, `root-pool-guest-normal-on/`
- `root-pool-off-concurrent-trace-stacks.json` (failure evidence)
- `root-pool-bench-summary.json`, `root-pool-bench-0-0/` through
  `root-pool-bench-3-0/`

## Complete cached installation with native fork preparation

The final diagnostic-fixed distribution was tested sequentially on two
independent roots initialized from the original seed. Both used portable fork
transport, the ordinary two-worker command pool, cached authentic archives,
and no fork/startup/I/O diagnostic logging. The native fork pool alone was
switched off/on. Both complete transactions installed the same 357 real
package versions and passed Node.js/npm, empty dpkg audit and empty file
verification. Ordinary scripts, triggers and synchronization remained enabled.
All 59 distribution hashes match and were rechecked after both runs.

| Measurement | Native pool off | Native pool on |
| --- | ---: | ---: |
| Complete installation, download excluded | 384.502 s | **283.206 s** |
| Local acquisition, reported separately | 2.637 s | 1.983 s |
| dpkg unpack / configure, one-second resolution | 370 / 7 s | 273 / 6 s |
| Job user / kernel CPU | 122.188 / 224.484 s | 117.469 / 214.734 s |
| Job total CPU | 346.672 s | 332.203 s |
| Native processes | 5,255 | 5,276 |
| Page faults | 41,695,601 | 41,744,219 |
| Post-install complete verification | 21.236 s | 14.922 s |

The observed wall-time reduction is 26.345%. This is a single off/on pair
with different host contention: 225 of 384 control samples observed other
compilers; none of 282 pool-enabled install samples did. Median host CPU was
49.3%/43.2%. The lightweight monitor used 1.703 CPU seconds across both complete
sessions. No compilation or other benchmark was started by this chat during
the pair. These observations prevent attributing the entire 101.296-second
difference to the pool. The separate alternating fork batches support a
specific fork benefit; the full result establishes installation correctness
and records the remaining **103.206 seconds above the requested target**.

The measured distribution includes other concurrently developed VFS, loader,
and mapping optimizations that had been integrated before freezing it. Its
hashes, not a claim of exclusive change attribution, identify these results.
Native preparation consumes each worker once and may discard stock when the
module template changes, explaining why total native process count need not
fall. Exec still creates a fresh native replacement process. No exec pool or
in-place image replacement is included.

Evidence:

- `artifacts/node-install-20260930/root-pool-off-install/report.json`
- `artifacts/node-install-20260930/root-pool-on-install/report.json`
- `artifacts/apt-180-20260930/root-pool-full-summary.json`
- `artifacts/apt-180-20260930/root-pool-install-pair.py`
- `artifacts/apt-180-20260930/root-pool-final-summary.py`
- `artifacts/apt-180-20260930/root-pool-off-host.jsonl`
- `artifacts/apt-180-20260930/root-pool-on-host.jsonl`
