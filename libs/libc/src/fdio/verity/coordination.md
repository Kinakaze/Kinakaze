# fs-verity VMA coordination: implemented foundation and remaining integration

This describes `coordination.rs`. It is not an ENABLE capability declaration.
`FS_IOC_ENABLE_VERITY` remains `EOPNOTSUPP`; neither ordinary mmap nor fork
creates a production `Channel`/`Member` yet.

## Protocol implemented and tested

Each physical inode selects a named pagefile root and dynamically allocated
membership pages. Every member pins its root and membership page. All shared
fields are atomics: immutable identity fields are initialized before a single
release-store publishes a slot; phase and epoch share one atomic word. A process
dying during membership-page extension leaves either an unpublished slot or a
published dead incarnation, both recoverable under the abandoned short mutex.

Membership identity is Windows PID + process creation time + a monotonically
allocated ticket. PID reuse, skipped tickets and stale acknowledgements cannot
remove or admit a different member. Access-denied while checking liveness is an
error, not proof of death. Native process-exit registrations wake the coordinator
without periodic polling or the 64-handle `WaitForMultipleObjects` limit.

The epoch owner holds a separate native mutex. Member waits never hold the short
state mutex or a mapping mutex. An inode-wide manual event is signalled before
PREPARING publication, so a coordinator dying before or halfway through the
per-member notification loop cannot strand an initially idle participant.
Native owner-mutex abandonment detects coordinator thread death as well as
process death. A live-thread marker distinguishes recursive mutex acquisition
from recovery and is cleared before a graceful owner releases its mutex.

The progression is PREPARING -> PARKED -> PUBLISHING -> COMMITTED. Existing
native members must acknowledge PARKED; a late mmap/fork member stays UNPUBLISHED
and cannot admit native pages during an active epoch. PUBLISHING is entered
before calling the durable backend; COMMITTED is published only afterward.
Abandoned PUBLISHING enters RECOVERING, never an assumed rollback. Its new owner
must inspect the durable inode record to choose COMMITTED or ABORTED. A parked
member is not admitted until its caller confirms verified installation or
native rollback; observing owner death alone never resumes memory access.

## Evidence

Run `cargo test --target-dir target/service-interface-check -p kinakaze-libc
--lib fdio::verity::coordination -- --test-threads=1 --nocapture`.

The tests execute hidden, bounded native child processes using disposable
create-new temp files. They cover live enrollment, stale epochs, late child
admission, participant death during a barrier, graceful unresolved publication
on the same thread, abrupt owner exit before EA publication, after a real durable
backend commit, and before any per-member event notification. Segment-extension
crashes are tested with the partially written segment pinned by another process;
recovery is not merely observing a fresh zero-filled section. They also test
process-creation-time mismatch and ticket ABA.

The native COW probe uses real file sections and `QueryWorkingSetEx`:

- Resident untouched pages have Shared=1; privately written pages have Shared=0.
- Writing the original byte back does not restore Shared=1; byte comparison
  cannot classify a page as clean.
- On the tested host, PAGE_NOACCESS makes Valid=0 even after VirtualLock. Shared
  must never be trusted when Valid=0.
- Restoring the original VMA's PAGE_WRITECOPY retains the private page. Its
  effective prior PTE protection may be PAGE_READWRITE, which is not necessarily
  legal to grant again on the readonly native file section.

Microsoft documents the resident-page prerequisite and Shared-bit distinction:
<https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualquery>
and <https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_ex_block>.

## Retained file VMA origin component

`fdio/file_origin.rs` now retains a duplicated, non-inheritable inode handle in
the managed arena, independently of the creating fd and pathname. The owner
records physical identity, original mapping address/length/file offset,
private/shared mode, initial Linux protection, and per-page current protection. Existing
split/replacement/rollback operations retain the appropriate references; an
anonymous MAP_FIXED replacement does not inherit the displaced file's origin.
The last reference unregisters its fork handle slot before closing/freeing it.

Private native, shared native, and verified mappings now use this inode owner.
Shared native views additionally retain their actual Windows section in a
separate managed `BackingRef`, with a registered non-inheritable handle slot,
section size and maximum view protection. The runtime's generic handle slots
duplicate into the actual child and patch the managed slots before provider
restore; the child checks its inode handle's physical identity. Libc mapping
handoff version 5 includes shared native views and preserves their source
geometry through nested fork. `ForkMappingStorage::RetainedSection` maps the
same kernel section into the child at the original file offset/address. It does
not allocate a pagefile replacement, copy bytes, zero shared data, or flush a
copy. Maximum legal creation permissions and current per-run protections are
separate. This path uses runtime CRYFORK4's 48-byte mapping record.

Ordinary private restored storage is still classified using the actual child
allocation (private allocation or pagefile section), not assumed to match the
parent's native view type. Verified shared data is immutable: its shared Linux
restriction lives in `verity::Info` while its verified cache storage uses
private native mappings, so it was and remains included in private-cache fork
restoration. Legacy shared anonymous mappings are not covered by the new
retained-file section path and still need their own ownership/restoration work.

Native COW, verified clean-cache, and copied-but-unclassified representations
remain distinct. In particular, fork and copy-based MAP_FIXED do **not** mark
all copied pages dirty. Until lineage is captured, native/copied file DONTNEED
returns EOPNOTSUPP rather than the anonymous path silently zeroing the pages.
Verified private DONTNEED continues to reload the verified clean snapshot.
Ordinary native section subrange unmap remains unsupported when Windows cannot
split the view; its EINVAL failure leaves the source handle and bytes intact.
This is an explicit remaining Linux behavior gap, not completed split support.
In particular, shared native views are not marked freely remappable: unmapping
an entire view to split it would temporarily revoke untouched neighbours from
concurrent guest readers. Retaining its section does not solve that quiescence
requirement.

In-process file growth through another fd now identifies private EOF
reservations by the retained inode, so fd reuse/path replacement cannot redirect
them. Growth preserves each page's protection. PROT_NONE reservations stay
inaccessible and materialize from the retained handle only when mprotect later
grants access; no temporary accessible view is published to other threads.
Cross-process external file-growth notification is not implemented by this
component. Its per-page protection metadata is not a COW dirty bitmap.

For an unpublished, non-fixed shared mapping, the view is created with the
original fd's allowed maximum and tightened before returning the address. This
preserves Linux's later MAYWRITE capability without granting write on readonly
fds. MAP_FIXED and EOF materialization intentionally do **not** use a transient
maximum-permission window at an already published address. A native probe
confirms the resulting host limitation: section creation PAGE_READWRITE=0x4,
NtQueryObject GrantedAccess=0xf0007 (includes SECTION_MAP_WRITE), then
MapViewOfFile3 PAGE_READONLY=0x2; later VirtualProtect PAGE_READWRITE fails with
Win32 87. The same section supports READ -> WRITE when an unpublished scratch
view initially has FILE_MAP_WRITE. Full fixed/EOF MAYWRITE therefore remains
blocked on a safe quiescent/atomic view transition, not fd or section rights.

Native evidence: `file_origin::tests` covers closed/unlinked/replaced source
paths, nonzero file offsets, partial protection transitions, EOF extension via
another fd, last-page rounding, verified discard/splits, anonymous replacement,
failure preservation, and 20 map/replace/split/unmap cycles with an unchanged
native handle count. `scripts/test-native-mapping-origin.ps1` builds all three
loader providers and runs a real static Linux ELF: fd close/reuse, rename and
path replacement, first fork, original-inode EOF growth, unlink, nested fork,
private data isolation, partial copied-storage unmap, and final retirement.
The runner uses disposable create-new files and bounded hidden child processes.
`scripts/test-shared-mapping-origin.ps1` adds two-way parent/child write
visibility, page-aligned non-granularity file offsets, closed/reused fds,
renamed/replaced/unlinked paths, readonly and PROT_NONE pages, nested fork, and
a further fork after unmapping both views to detect stale retired handle slots.
The first successful real shared run was one iteration / three actual forks on
the preceding WriteProcessMemory arena-transfer baseline; the new arena
transport requires its own complete three-provider rebuild and rerun.

## Exact integration still required before removing the existing gate

1. **Runtime quiescence.** A common gate must cover pthread creation, raw clone,
   thread exit, mmap mutation and fork. The existing private fork freezer suspends
   host threads and is not a safe standalone Guest-quiescence API. Preallocate
   conversion and rollback resources, establish quiescence, sample COW while
   pages are resident/readable, then revoke native access. Only then acknowledge
   PARKED. Merely VirtualProtect-ing pages is not a quiescence proof.
2. **Retained VMA identity.** The private/shared file handle, section, geometry
   and protection component above is connected. Enrollment ownership and a
   real COW bitmap remain missing. Complete native partial unmap/MAP_FIXED and
   native/copied DONTNEED behavior must preserve that identity and dirty state;
   a conservative unsupported result is not completion. Enrollment may retire
   only after the last native view has actually gone.
3. **Fork handoff.** Private and shared native file-origin VMAs now retain and
   restore their inode/section handles and geometry, including nested fork.
   Legacy shared anonymous ownership still needs coverage. Before Guest resume,
   the child obtains its own ticket, checks the durable EA and current epoch,
   and installs or waits; it never inherits a parent's LockFileEx lease. This
   pre-resume rule must hold even when the parent dies before a ready handshake.
4. **Fork COW lineage.** Native private mappings become anonymous/pagefile
   storage during existing fork restoration. Their new Shared bit alone no
   longer describes which original-file pages were privately dirty. Capture and
   serialize that dirty bitmap while the parent is quiescent and preserve a
   detectable first-write boundary for the child's originally clean pages.
   Treating every fork page as private would bypass verification.
5. **Real conversion and faults.** Stage verified clean data plus retained
   private-dirty bytes, install exact-address mappings, preserve EOF/corrupt-page
   SIGBUS and original PROT_NONE SIGSEGV, and publish lock-free transition fault
   ranges. Fault waits need independently retained completion handles; only
   installed/rolled-back mappings may return loader action -1 (retry RIP).
   The current callback intentionally still returns only 0 or SIGBUS.
6. **VFS transaction boundaries.** Connect the existing split backend
   prepare/build/commit to the real VMA barrier so the deny-write reservation
   remains held across global PARKED, without holding an
   inode mutex needed by participant reads/conversion while waiting for them.
   Durable EA recovery decides an interrupted publication. Preserve Linux
   EINTR/cancellation and fail closed on permission, storage or coordination
   errors; success cannot precede complete barrier coverage.
   Abort that removes hidden tree storage/padding must first retire every old
   native section and view, including readonly views: PAGE_NOACCESS revokes
   access but does not release Windows' section-size pin, so shrinking EOF can
   still fail with EIO. Rollback must recreate/adopt views only after restoring
   the original physical size; parked protection alone is insufficient.
7. **Namespace/security and migration.** The foundation follows the existing
   VFS inode lock's `Local\\` Windows-session scope. Cross-session access and
   object DACL policy need one consistent solution, not an invisible second
   registry. Existing processes without enrollment cannot be silently declared
   safe; production activation requires a controlled rollout/restart boundary.

None of these missing integration contracts is replaced by a process-local
registry check, by rejecting every already mapped file, or by adding a broker.
