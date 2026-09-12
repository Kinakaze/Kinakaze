# Explicit retained-handle fork handoff

`register_fork_handle_slot` registers a stable managed-arena `AtomicUsize`, not
an inheritable raw handle or a process-local Rust pointer. The executable's
coordinator owns the registration set; DLL copies forward through mandatory
process exports. The owner must unregister before freeing the slot or closing
its native handle, while holding the mapping transaction.

Fork holds that transaction from participant serialization through materializing
the child. It duplicates every registered handle into the prepared child with
`bInheritHandle=FALSE`, copies the arena, then patches the child's slots with the
actual values returned by `DuplicateHandle`. This precedes mapping restoration,
context installation and all child callbacks. Failed duplication/copy terminates
the unpublished child, whose handle table owns already-created duplicates.

The strict `CRYFORK3` frame includes the slot-address set so a restored process
can fork again. There is no old-frame fallback. The child must adopt the owning
objects/reference counts through its existing participant. Registering a handle
does not restore an omitted/shared VMA, copy a byte-range lock, or clone a socket,
IoRing or IOCP. Those require their own resource semantics.

The topology transaction now also covers pthread/raw-clone native thread
publication. Raw clone releases it before waiting for child initialization, which
can itself require VM/TLS mappings. This closes thread-publication and mapping
snapshot races; it is **not** a general safe guest-quiescence API. Thread-exit,
asynchronous host access, COW classification and old-view conversion still need
the integration described in libc's fs-verity coordination document.

The native helper test checks a real target process: after patching its slot and
closing the source handle, the target still sees the signalled kernel event and
a non-inheritable handle. Invalid registrations, duplication failure and strict
handoff parsing have separate tests. This helper alone does not exercise the
complete runtime fork stack/arena path; the native-file-origin ELF probe covers
that integration and nested fork.

Windows ordinary private allocations reject section-only WRITECOPY protection.
The fork copier translates that permission to READWRITE (and the executable
variant to EXECUTE_READWRITE) only when the child storage is Ordinary, preserving
modifiers. This preserves access rights, not the original file's dirty-page
lineage; file-origin metadata keeps that lineage explicitly unclassified.

Native contracts were checked against Microsoft's
[DuplicateHandle documentation](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-duplicatehandle)
and [memory protection constants](https://learn.microsoft.com/en-us/windows/win32/memory/memory-protection-constants).
