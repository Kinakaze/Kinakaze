# Native fork preparation

Sessions started with both `KINAKAZE_FORK_TRANSFER=1` and
`KINAKAZE_FORK_POOL=1` may consume a prepared native worker for an internal
Linux fork. The session must use the existing command-pool configuration;
that pool and the native fork stock remain separate. Both switches are opt-in.

Init owns at most two unused workers for the current native module/TLS
template. Their modules are pinned against replacement and their main threads
block on an activation event. Consumption wakes the bootstrap, waits for its
user-mode ready handshake and suspends it before guest memory is installed.
Each worker is used once. A miss creates an ordinary native child immediately.

An explicit participant contract gates the portable path. Unknown participants,
legacy hooks, native console handles and IoRing descriptors select ordinary
inheritance. The audited core participants restore guest values, stable object
identities or registered native-handle slots. VFS frames index native handles
and duplicate them into the selected child while descriptor/mapping owners are
pinned. Native socket handles retain their Winsock-specific transfer protocol.
The vfork rendezvous events are also transferred explicitly. Retried native
address-space placement starts from original source handles.

The manager authorizes delivery only for the calling worker's prepared fork
transaction. Init retains the unpublished child and pinned parent identity until
commit; abort, native parent death or failed reply delivery terminates the
candidate. Session shutdown covers unused and consumed children through its
Job. Pending deliveries are limited to 128. Successful fork adoption uses the
existing Linux PID, activation and process-exit protocols.

Frozen-copy diagnostics use a bounded host-private buffer and flush after both
allocator freezes end. This prevents stderr locking from deadlocking with a
frozen sibling that is waiting for allocation.

The implementation is intentionally isolated from the concurrent VFS metadata,
I/O routing, native catalog, provider metadata and write-watch optimizations.
The complete-install measurements in
[the retained JSON](measurements/native-fork-pool-2026-09-30.json) use an earlier
frozen **integrated** distribution containing those optimizations. They do not
measure this feature branch's standalone binary. The integrated pool-enabled
transaction passed in 283.206 seconds, excluding download, so the requested
180-second target remains unmet. The fuller evidence and limitations are in
[the performance record](fork-provider-performance-2026-09-30.md).

Focused coverage includes independent native handle tables, shared file offsets,
FIFO/Unix/UDP state, partial-transfer rollback, fork-participant ownership,
manager reservations, live child termination and commit survival. The packaged
probes additionally check nested fork/exec, CLOEXEC, mapping isolation, signals,
current cwd/environment and fallback after loading an unaudited participant.
Nonempty portable overlay/mount transfer needs further dedicated coverage
before enabling the optimization generally.
