# R11 native write-open performance

The frozen R11 Release completed the complete 357-package Node.js/npm installation
in **224.236 seconds**. Node.js, npm, dpkg audit and complete installed-file
verification passed. Verification took a separate 12.819 seconds.
Installed names and versions exactly match R10. The 180-second target remains unmet by 44.236 seconds.

Existing ordinary native files can now acquire a writable descriptor through one
namespace lookup and classify the same retained inode. The path rejects native
reparse points, hosted links and special inodes; complex names, creation,
truncation, append and callers without DAC override retain the full resolver.
The selected mount writer, live permission check and fs-verity write guard remain
mandatory. Descriptor access flags, shared offsets and inode ownership survive
rename, unlink, duplication, fork and exec through the existing description path.
Set `KINAKAZE_NATIVE_WRITE_OPEN=0` to disable the new path.

This targets a concrete dpkg operation: version 1.21.23 reopens extracted files
with `O_WRONLY` for its deferred writeback barrier and fsync. Both synchronization
calls, maintainer scripts and triggers remain enabled. The reference is the
[official dpkg 1.21.23 source archive](https://deb.debian.org/debian/pool/main/d/dpkg/dpkg_1.21.23.tar.xz),
with the inspected source and hashes retained alongside the measurements.

## One-distribution comparison

Four sessions ran in disabled/enabled/enabled/disabled order. Each session recreated
512 files before timing, exercised repeated opens, varied opens and opens followed
by a seven-byte write, then checked every file's contents. No build or other
benchmark from this task ran concurrently. Host load was uncontrolled.

| Loop | Disabled mean (ms) | Enabled mean (ms) | Reduction |
| --- | ---: | ---: | ---: |
| repeated | 2653.576 | 1396.086 | 47.39% |
| varied | 2191.303 | 1167.113 | 46.74% |
| varied_write | 3533.889 | 2010.073 | 43.12% |

These are local-operation results, not a prediction of the complete-install gain.

## Complete-install measurement

| Measurement | Result |
| --- | ---: |
| Installation | 224.236 s |
| Job CPU | 314.234 s |
| Job user / kernel CPU | 108.297 / 205.938 s |
| Native processes | 5,392 |
| Page faults | 43,269,044 |
| Complete installed-file verification | 12.819 s |

The installation used a fresh root from the same fixed seed as R10. The 30-second
quiet-start gate was met; foreign build/runtime
activity occurred in 104 of
112 installation samples. The observer used
0.281 seconds of CPU and 0.327
seconds of sampling wall time. This task performed no simultaneous build, root
setup or separate benchmark. Full-run differences from R10 include host variation.

## Validation and provenance

The seven-file change was built on the frozen R10 source, itself based on revision
`42d04ec1727bcae07fd266d79b4df99b8b43f1b6` plus the two regenerated export definitions
documented in R10. R11 contains 1,827 source files and 61 distribution files.
The normal Release build and export checks passed; source and distribution hashes
were rechecked after validation and installation.

All 228 top-level native tests passed, with four existing tests ignored, and all
68 packaged guest checks passed across disabled, ordinary-fork, transferred-fork
and pooled-fork variants. New cases explicitly require successful use of the fast
path, then check access flags, retained inode writes and fs-verity exclusion in
both directions. Guest cases additionally cover permission changes, non-root
denial, readonly bind mounts, links, FIFOs, append and truncation fallbacks.

[Exact measurements and evidence hashes](measurements/native-write-open-r11-2026-10-01.json) retain every
phase, ABBA sample, runtime flag, test record and host observation. Raw evidence
is under `artifacts/goal-install-180-20260930/`.
