# Session-owned native provider catalog

The init command pool retains validated native declarations in a read-only
section, with deny-write/delete file references for every source image. Worker,
fork and exec startup can receive that section and its lifetime references from
the authenticated session instead of reopening and parsing all provider files.
This integrates the existing workspace catalog implementation with native fork
and exec preparation and deferred fork provider metadata.

Each request is restricted to the session's configured native directory and
an authorized, live worker identity. Directory names are checked on each
request; additions cause new discovery. Symlinked distributions and mixed
writable ELF/native directories use ordinary discovery. Malformed additions
still fail validation. The receiver validates bounds, names, ordering, module
identities and the exact number of source references. Source references survive
deferred binding and catalog replacement. Failed replies roll back transferred
handles against the original process incarnation.

`KINAKAZE_NATIVE_CATALOG=0` selects ordinary discovery in the same binary.
Configurations without a compatible init pool retain that path as well.

The integrated release passed 137 native tests, with two subprocess fixtures
ignored, and 13 packaged guest checks: ten with sharing enabled and three
fork/exec/provider checks with it disabled. Both native preparation pools were
enabled. Idle stock added zero Job CPU milliseconds during the one-second check.

Four unprofiled off/on/on/off runs each completed 128 operations per path:

| Median guest wall time | Ordinary discovery | Shared catalog |
| --- | ---: | ---: |
| Pure fork | 2.085 s | 1.694 s |
| Fork followed by exec | 4.470 s | 3.587 s |
| posix_spawn | 3.967 s | 3.499 s |

Both shared-catalog samples were below both ordinary samples for every path.
The observed fork+exec reduction was 19.8%; these short workloads do not predict
the full installation saving. All four samples and host observations are kept.

A separate diagnostic pair recorded 32 forks in each mode. Registry restoration
medians were 8,356 us ordinary and 3,396.5 us shared. The enabled trace recorded
one catalog miss followed by 49 hits. The small hit span alone excludes directory
enumeration and capability transfer; it is not the whole request cost.
Diagnostics were excluded from the timing comparison.

The measurement JSON contains source and distribution hashes, native and guest
validation references, all timing rows and diagnostic spans:
[shared catalog measurement](measurements/shared-native-catalog-2026-10-01.json).
This catalog-only integration has not yet received a complete-install timing;
the 180-second goal remains unmet.
