# Native exec preparation

`KINAKAZE_EXEC_POOL=1` enables an optional supply of two unused exec workers in
sessions configured with the existing init command pool. The switch is separate
from native fork preparation. A miss or unsupported portable VFS state keeps the
ordinary exec launch path. The feature is not enabled by default.

An unused worker loads the runtime DLL and resolves its entry points, then
blocks on a private activation event **before RuntimeOpenV1**. It has no Linux
PID or manager session. Init pins distribution images against replacement and
assigns each suspended process to the session Job before resuming bootstrap.
Creation runs outside the manager mutex, with a separate shutdown gate.

For a successful claim, init duplicates exactly three capabilities into the
authenticated worker: the candidate process, activation event, and bounded
control mapping. The parent owns a portable exec snapshot before claiming stock.
It sets the current executable and host cwd, transfers the snapshot's native
capabilities, and reserves the existing Linux identity before activation. The
guest argv, environment, descriptors, cwd, root and signal state travel in the
existing VFS frame. Executable bytes are not copied again when transferring its
capability table.

The replacement follows the ordinary readiness, ownership and exec commit
handshakes. It cannot execute the new guest image until the manager observes
the original native process's death. Each prepared worker is consumed once.
No guest address space or descriptor table is recycled.

Init retains custody while an exec reservation is pending. Failed reply delivery,
abort, candidate death or native parent death revokes the unpublished candidate.
Commit releases that custody before parent liveness is checked. A delivered
candidate that never gets a reservation expires after 30 seconds; outstanding
deliveries are bounded to 128 and one live candidate per parent identity.
Unused workers wait without polling and are covered by session shutdown.

Validation on 2026-10-01:

- 111 protocol, manager and init native tests passed; two subprocess fixture
  entry points were intentionally ignored. This includes live-process rollback,
  parent death, delivery cancellation, abandonment and commit survival.
- Eight packaged guest probes passed with exec preparation disabled and enabled
  against the same frozen distribution. They cover nested fork/exec, concurrent
  vfork, descriptor transfer, shared futexes, allocation and Unix rights.
- A separate trace observed 25 prepared exec activations while preserving the
  Linux PID, cwd/environment, redirected stdout, CLOEXEC, file offset, pipe,
  Unix socket, UDP binding, eventfd and signal state. Nested fork/exec also passed
  with both native preparation pools active.
- Idle inventory contained exactly two native exec workers, while the manager
  counted only its two ordinary command workers. The measured one-second idle
  interval added zero Job CPU milliseconds.

Trace timings are functional evidence only. Complete-install performance is
measured separately with diagnostics disabled and cached downloads accounted
for outside installation time.

The first unprofiled alternating off/on/on/off comparison ran 128 iterations
per path. Median fork+exec wall time was 8.082 seconds without exec stock and
6.144 seconds with it (24.0% lower). Both samples with exec stock were below
both samples without it. However, unchanged fork and posix_spawn controls had
large adjacent-run fluctuations, so this is not an isolated estimate of the
whole-install saving. All four rows, Job counters, binary hashes and source
hashes are retained in [the measurement JSON](measurements/native-exec-pool-2026-10-01.json).
The complete cached installation target of 180 seconds remains unverified for
this distribution.
