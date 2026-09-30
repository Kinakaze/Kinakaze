# Portable exec snapshot ownership

`prepare_portable_exec_state_from_image` prepares VFS state for an unpublished
replacement that has an independent Windows handle table. The optional
[native exec pool](native-exec-pool.md) uses this ownership contract. The snapshot
API itself does not establish an installation performance improvement.

Each native handle is duplicated into a non-inheritable local owner while its
serializer still holds the source table lock or object reference. Repeated
numeric handle values must identify the same kernel object. A descriptor
generation check rejects close/open/dup changes during serialization. After
successful capture, closing or recycling original descriptors cannot change
the objects transferred to the replacement. The transfer patches only the
capability table and keeps the executable bytes in place.

Winsock uses its provider duplication protocol to retain local socket references
until the destination consumes its own records. Unix socket serialization pins
the pipe under the descriptor lock and includes only retained descriptors, so
CLOEXEC side-state handles do not leak into the replacement. Borrowed standard
streams become owned capabilities; unsupported console or IoRing objects return
an error for the coordinator to handle.

The caller must serialize image transactions, keep the destination stopped until
transfer succeeds, and either restore the frame once or terminate that process.
`PortableExecState::finish` completes the existing Unix/Winsock handshake; dropping
an unfinished snapshot cancels it and releases local references. A failed remote
transfer rolls back all capabilities created by that attempt.

Focused native tests exercise real cross-process file offsets, pipes, FIFO,
Unix sockets, UDP, eventfd, CLOEXEC and borrowed stdio after the source descriptors
are closed and reused. Other cases check native handle reuse, non-inheritance,
error/panic cleanup and cancellation. Worker integration and complete-install
measurements are documented separately.

Validation on 2026-10-01: release native tests passed for `native_transfer::tests`
(10) and `exec_` (18, one overlapping test), totaling 27 distinct tests. The
tests and their SONAME dependencies were staged from one successful Cargo JSON
build with `tools/stage-native-test-artifacts.py` before execution.
