# R10 complete installation

The frozen R10 Release completed the full 357-package Node.js/npm installation
in **215.415 seconds**. Node.js, npm, dpkg audit and complete package file
verification passed. Verification took another 10.682 seconds. Installed
package names and versions exactly match R9. The 180-second target remains
unmet by 35.415 seconds.

R10 starts from committed revision `42d04ec1727bcae07fd266d79b4df99b8b43f1b6`.
The initial build correctly rejected stale generated exports. Regenerating
`libs/libc/exports.def` and `libs/libpthread/exports.def` from that build's
libraries refreshed duplicate native declarations and the placement of shared
mutex declarations. No runtime source changes were made to that revision.
The final source manifest records those two generated differences explicitly;
the original Git archive, first failure and successful normal build are retained.
The successful packaging check covered 29 modules and 5,971 guest exports.

The frozen source contains 1,824 files. Validation passed 224 top-level native
tests, with four existing tests ignored; 64 packaged guest checks;
four shared/robust pthread probe groups; and 24 export-generator tests.
The guest checks cover read-open controls, absolute and relative names,
permissions, namespaces, inode and descriptor lifetime, fork/exec modes, and
exact robust-mutex symbol versions. All 61 distribution hashes were checked
before and after validation and the installation.

| Complete-install measurement | Result |
| --- | ---: |
| Download preparation, separate from installation | 2.173 s |
| Installation | 215.415 s |
| Job CPU | 293.219 s |
| Job user / kernel CPU | 104.047 / 189.172 s |
| Native processes | 5,391 |
| Page faults | 43,382,567 |
| dpkg unpack / configuration, one-second resolution | 201 / 5 s |
| Complete installed-file verification | 10.682 s |

Maintainer scripts, triggers and synchronization operations remained enabled.
No build or separate benchmark ran in this task during installation. The quiet
start condition was not met; foreign build/runtime activity was observed in
25 of 108 installation samples. The observer used 0.297 seconds of CPU and
0.245 seconds of sampling wall time over the driver run. Source and host load
differ from R9, so the full-run difference cannot be assigned to one change.

[Exact measurements and evidence hashes](measurements/goal180-r10-integrated-2026-10-01.json)
include source provenance, generated-file differences, every phase, distribution
hashes, test records and host observations. Raw evidence is retained under
`artifacts/goal-install-180-20260930/`.
