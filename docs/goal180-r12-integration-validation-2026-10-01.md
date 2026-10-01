# R12 integrated validation

The Release built from committed revision `ee72c47c84e96f639a04fbf68b640ed5e73a00c7`
passed 243 top-level native regressions (7 existing tests ignored),
72 packaged guest checks, eight pthread probe groups and 25 export-generator tests.
The tests cover the combined native read, existing-write and exclusive-create
paths, descriptor and inode lifetime, permissions, namespaces, fork/exec modes,
shared mutexes and conditions, robust recovery and shared robust cancellation.

The initial export check found stale linker definitions. Regenerating only
`libs/libc/exports.def` and `libs/libpthread/exports.def` produced the accepted
source. The successful normal build checked 29 native modules and 5,973 guest
exports. All 1,853 source and 61 distribution hashes were checked after validation.
The source archive and both original and regenerated manifests are retained.

This is validation only. The next fresh root was prepared, but no R12 full
installation was launched before the user's request to wrap up and push Git.
The 180-second target remains unmet. The preceding measured native-write result
and its host-load limits remain recorded in [R11](native-write-open-r11-2026-10-01.md).

[Evidence hashes](measurements/goal180-r12-integration-validation-2026-10-01.json) identify the exact build and tests.
The first native-test launcher ended without a result; only the completed retry
is included in the accepted test counts.
