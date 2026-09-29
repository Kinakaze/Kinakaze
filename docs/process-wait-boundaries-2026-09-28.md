# Process wait and lifetime boundaries, 2026-09-28

## Confirmed defects and changes

`engine/crates/kinakaze-runtime/src/lib.rs`:

- Process-table initialization and ordinary table access previously continued
  after failing to acquire the Windows mutex. They now fail without touching
  shared table contents. An RAII guard also releases the lock during unwind.
- Wait candidates previously borrowed raw handles from the child registry after
  releasing its lock. Concurrent reap/exec could close those handles. Each wait
  now owns a noninheritable reference until that candidate is discarded.
- A concurrent waiter consuming an exit report could be mistaken for a native
  death without an exact Linux status. Removed/replaced rows and consumed zombie
  reports now cause a rescan, not a fabricated integrity error.
- Child-list snapshots can outlive a reap or exec. Candidate collection checks
  identity again and distinguishes disappeared rows from actual open failures.
- Opening a handle from a stale snapshot used to reinsert it into the fork
  registry after another waiter removed it. Once the shared row disappeared,
  that slot was no longer collectable. Enough such races exhausted the 64-slot
  local registry and caused unrelated `fork` calls to return EAGAIN. Newly
  opened wait references now belong solely to the wait and are never inserted
  into that registry.

The shared process table remains retained by init. Per-wait owned handles are
temporary references, not new authoritative worker-owned state. No polling
interval was enlarged to hide races.

## Regression and evidence

`tests/guest/ProcessExitBoundaryProbe.py` checks exact exit codes across fork and
exec, repeated WNOWAIT, pipe EOF after reap, WNOHANG, rejection of a second reap,
four concurrent reapers of eight children, and SIGTERM/SIGKILL status.
`tools/test-process-boundaries.py` runs it under an owned InitPool, then existing
SIGCHLD payload/stop/continue/exec coverage and repeated real tar/gzip round trips.
Optional manager sampling writes a separate JSONL file.

- Original release `goal-daily-optimized-release`: concurrent reapers reproduced
  three EIO errors (`artifacts/goal-process-concurrent-baseline`).
- Intermediate fixes passed 32 rounds, but larger tests exposed further races;
  those runs must not be counted as final success.
- `goal-process-boundary-registration-trace` identified the cumulative failure
  as `child-handle registration failed`, consistent with stale reinsertion.
- The earlier unrelated tar `waitpid/ECHILD` report is not proven to have this
  same cause. Passing archive stress alone does not establish its root cause.

## Final validation

- Development distribution: `artifacts/goal-process-boundary-dev`.
- Optimized distribution: `artifacts/goal-process-boundary-final-release`
  (native runtime for an existing Debian root, not a fresh-install archive).
- Final kernel suite: 81 passed, 1 auxiliary child test ignored in the ordinary
  listing and exercised by its parent test. Log:
  `artifacts/goal-process-boundary-final-dev/kernel-tests.log`.
- Final development stress: 128 exact exit/exec cases, 128 batches of eight
  children with four concurrent reapers (1,024 children), signal and SIGCHLD
  coverage, and 128 tar/gzip round trips all passed.
  `artifacts/goal-process-boundary-final-dev/results.json`.
- Final release process/signal tests and all 128 archive round trips passed at
  the same scale: `artifacts/goal-process-boundary-final-release-check/results.json`.
- Init custody passed, including native worker forced death, preserved tmpfs
  contents, sysfs reuse, namespace isolation and final unmount cleanup:
  `artifacts/goal-process-boundary-final-ownership/result.json`.
- Final release proc/filesystem boundaries, Bash interrupt and deterministic
  offline edit/test workflow passed:
  `artifacts/goal-process-boundary-final-compatibility/report.json`.
  This is not an online model or latest Claude/pi compatibility claim.

These tests verify the reproduced EIO and cumulative EAGAIN races; they do not
prove the absence of every scheduling bug. The local fork registry still has a
64-concurrent-child capacity, distinct from the fixed cumulative slot leak.
