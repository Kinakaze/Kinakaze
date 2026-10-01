# Exclusive-create parent permissions

The ordinary native `O_CREAT|O_EXCL` branch could create a directory entry
without checking Linux write/search permissions on its selected parent.
An unprivileged forked guest reproduced the failure with the local speculative
create path both enabled and disabled. That local duplicate implementation
was archived when the independently validated native-create and native-write
optimizations landed in main; the final change adopts those implementations.

The existing retained parent and metadata now authorize creation before the
atomic native create. Filesystem IDs, supplementary groups and DAC
capabilities select the applicable permission bits. Search is required even
for an existing leaf. Without parent write permission, a searchable existing
leaf returns `EEXIST`; an absent leaf returns `EACCES`. The denied-write case
uses a non-mutating lookup relative to the retained parent. No file is created
to test permission. The new file's own mode continues to govern future opens,
so an authorized mode-zero create still returns its requested writable fd.

This is a focused check on the selected parent of native exclusive creation,
not a replacement of the general path walker. It adds no metadata open to
authorized creates and leaves the upstream DAC-override fast path in place.
Scripts, triggers, range writeback and full fsync remain enabled.

## Validation

The coherent Release build based on `48067166ec504b4d9ff032ad24970252d6418928` plus the five
hashed source files passed 20 packaged guest probes. They cover exclusive
creation, owner/group/other grants and denials, existing-leaf error precedence,
setgid inheritance, mode-zero writable fds, retained rename/unlink identity,
exclusive-create races, readonly bind mounts, native read/write paths,
filesystem boundaries, credentials, namespaces, fork/exec pools, PI futexes,
shared/robust pthread synchronization and allocation. The exclusive-create
permission and existing lifecycle probes also passed with
`KINAKAZE_NATIVE_EXCLUSIVE_CREATE=0`, for 22 passed guest checks total.
All 59 distribution hashes were rechecked unchanged. The idle observation
used 0.000 CPU ms over one second with bounded workers.
All 25 export-generator tests passed; the packaged release passed checks for
29 modules and 5,973 guest exports. The two regenerated export definitions
only relocate existing condition-attribute declarations into generated order:
their complete nonempty line multisets are identical to the definitions used
by the build. No public/native name, version or forwarder changed. This
equivalence and the final export check are recorded alongside the binaries.
Static entry sources and their dependencies
have no changes since the previously validated catalog entry build.

The failed initial on/off probes and final successful results are retained
with hashes in [the measurement record](measurements/exclusive-create-parent-permissions-2026-10-01.json).
The local unmerged optimization and interrupted build artifacts are preserved
under `artifacts/apt-180-20260930/root-native-create-*`; their native test counts
are not attributed to this final integrated source.

The independently verified complete-install best remains **204.887 seconds**,
with downloads measured separately, as recorded in
[the integrated installation report](integrated-native-read-install-2026-10-01.md).
No new complete-install speedup is claimed for this permission repair, and the
requested sub-180-second target remains unmet. Upstream create and write-open
performance evidence remains in its existing reports and has not been replaced.
