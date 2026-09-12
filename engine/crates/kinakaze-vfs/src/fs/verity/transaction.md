# fs-verity storage transaction contract

This API implements the durable inode transaction, not a complete VMA barrier.
The production `FS_IOC_ENABLE_VERITY` gate is intentionally unchanged.

## Ownership and ordering

1. Pin the ioctl inode with `Opened::from_fd`. `Opened::prepare` validates the
   parameters, acquires a native deny-write reservation, recovers any orphan
   record, and flushes a new PREPARING record with a random 128-bit identity.
2. Keep the returned `Transaction` alive while the coordinator establishes the
   global PARKED barrier. No transaction method returns with the inode mutex
   held. No guest handler runs inside these methods.
3. Only after all native VMA participants are inaccessible/quiescent, call
   `build`. It streams the Merkle tree with bounded storage, flushes the tree,
   then writes and flushes BUILT including its final descriptor. For 1024/2048
   byte Merkle blocks, the hidden tail can occupy the original native EOF page;
   parking only immediately before `commit` is too late.
4. Peers retain their own native inode handles and use the serialized
   `TransactionId` with `staged_descriptor` / `staged_verified_read`. A Rust
   transaction or numeric handle is not a transferable cross-process identity.
   Each read uses a separate asynchronous handle, clips at logical EOF, verifies
   data before copying, and rejects stale or not-yet-built identities. A short
   read must be handled like a normal short read; a verified prefix is not proof
   that subsequent blocks are valid. Successful staging alone never authorizes
   a peer to admit pages without the epoch's installation decision.
5. Enter the coordination protocol's publication phase, then call `commit`.
   It replaces BUILT with ENABLED and flushes. Only successful commit proves this
   owner's durable publication. Retain the transaction through participant
   installation/resume; commit does not release the native write reservation.
6. On unpublished failure, retain rollback data and retire the original native
   views/sections before `abort`; only then recreate/admit original native views.
   PAGE_NOACCESS is not sufficient: the native regression with EOF=17003 and
   1024-byte Merkle blocks permits append but rejects shrinking the still-mapped
   EOF-padding page. `abort` truncates and flushes hidden bytes before
   removing and flushing the record. If rollback fails, retain the reservation
   and keep participants parked until cleanup is resolved. Restoring native
   views before the storage abort can prevent that cleanup from ever succeeding.

Errors retain the native reservation. Drop only closes its handle: it does not
perform blocking cleanup, retry I/O, dispatch a signal or report success. A
dropped/process-dead unpublished transaction leaves a masking record for later
recovery. Guest signal delivery belongs outside the operation, after the caller
has safely resolved/released its coordination resources.

## Publication and recovery are not inferred from errno

`TransactionState::Published` means ENABLED is visible but its final flush has
not succeeded. It cannot be aborted; keep the reservation and retry `commit`.
`Indeterminate` means metadata inspection is required. `reconcile` updates this
owner without releasing its reservation. It flushes a visible BUILT record and
never upgrades a visible ENABLED record to Committed by observation alone.

`publication(handle)` returns the visible authoritative EA state, not evidence
that a failed flush reached stable storage. After winning global epoch recovery,
`Opened::resume(expected_id)` reacquires deny-write and retains the same identity.
A live owner blocks this operation. An identity mismatch is ESTALE, and ENABLED
returns Published, requiring a new successful commit flush. The coordinator, not
the backend, decides whether unpublished PREPARING/BUILT should resume or abort.
Resumed PREPARING is flushed before any new tail append. A partially completed
abort may leave BUILT after its tree was truncated: resume retains that owner as
Indeterminate, and reconcile/commit cannot declare a missing tree complete.
Malformed and unknown records fail with EIO, never as ordinary-file absence.

Native sharing protects active PREPARING/BUILT from `ensure_writable`: its recovery
writer cannot open while the transaction owns deny-write. Conversely, an open
recovery writer blocks a new transaction's reservation. The inode mutex serializes
the recheck/truncate/record-clear sequence between recovery attempts.

## Persistent format

One strict format is supported: `CYVERIT2`, 320 bytes. The 64-byte header contains
the transaction phase, tree geometry and 128-bit ID; the remaining 256 bytes are
the Linux fs-verity descriptor unchanged. PREPARING=1, ENABLED=2, BUILT=3. Reserved
header bytes must be zero. Legacy `CYVERIT1`/288-byte fixtures are not accepted:
production ENABLE was never released, and no compatibility fallback is added.

## Verification and boundaries

Tests use actual native EAs, file data, native sharing and bounded hidden child
processes. They cover active-owner exclusion, independent staged readers,
transaction-ID ABA, corruption, rollback ordering, fatal-signal observation, and
abrupt process exit at PREPARING/BUILT/ENABLED boundaries followed by explicit
resume/abort or resume/commit. Per-owner test-only checkpoints inject EIO/EINTR
around real operations; these are not claims of inducing hardware power loss.

The runtime must still cover every existing/new/forked VMA with quiescence,
retained identity, COW lineage, verified installation and transition-fault
handling. Active transient owners must not be blindly copied through fork.
The current inode-lock namespace is Windows-session-local. Host readonly files
still fail preparation with EACCES instead of temporarily weakening attributes.
The storage-only `enable` convenience function remains for existing backend and
coordination tests; it is not authorization to bypass those production contracts.
