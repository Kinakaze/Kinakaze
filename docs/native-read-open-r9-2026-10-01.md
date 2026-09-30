# R9 absolute native read-open measurement

The R9 Release distribution completed the full 357-package Node.js/npm
installation in **255.751 seconds**. Node.js, npm, dpkg audit and full package
file verification passed. Verification took another 10.698 seconds. Installed
package names and versions exactly match R8. The 180-second target remains
unmet by 75.751 seconds.

This frozen candidate allows ordinary absolute read-only opens to retain a
single native data handle. The native open rejects reparse points throughout
the path, and the same handle supplies the inode record, permission check and
installed descriptor. The scope requires the existing DAC override capability
and excludes mount crossings, hosted links, special files, directories and
complex path components. Other cases use the existing resolver. The handle has
ordinary read access; the shared native-open helper retains separate backup
semantics for metadata operations. No path or inode record is cached across
calls.

The new guest test exposed a pre-existing data-open bug: a regular filename
followed by `/` was accepted, including with the optimization disabled. Data
opens with a trailing slash or order-sensitive dot components now use the
existing component walker before pathname normalization. Tests cover this
case, a directory link followed by `..`, and a directory link with a trailing
slash and `O_NOFOLLOW`.

Four sessions used the same R9 distribution with
`KINAKAZE_NATIVE_READ_OPEN=0,1,1,0`. Each used ordinary root-owned files with
mode 0640. File creation was outside the reported loop times. The varied cases
cycle through 512 names and check read contents. Fork transfer, fork preparation
and exec preparation were enabled in all four sessions.

| Loop | Control 1 | Enabled 1 | Enabled 2 | Control 2 |
| --- | ---: | ---: | ---: | ---: |
| 10,000 open/close calls, one file | 1,551.457 ms | 640.778 ms | 593.237 ms | 2,176.234 ms |
| 8,192 open/close calls, 512 files | 1,367.327 ms | 524.580 ms | 542.627 ms | 1,745.073 ms |
| 8,192 open/read/close calls, 512 files | 2,286.372 ms | 1,125.261 ms | 1,185.276 ms | 2,477.700 ms |

The complete guest command, including fixture setup and Python startup, used
5.625, 2.797, 2.766 and 6.906 seconds of Job CPU respectively. Host load was
uncontrolled. This is evidence for the measured read-only workloads, not a
prediction of installation savings or a measurement of writable opens.

The normal Release build passed 213 distinct native tests, with three existing
diagnostics ignored, and 56 packaged guest checks. Two child-process fixture
executions are recorded separately rather than added to the native test count.
The six new native tests include an explicit successful fast-path descriptor
installation, native junction rejection, inode retention after rename/unlink,
namespace and escaped name handling, and special-file/flag fallback. The guest
matrix covers the optimization disabled and enabled with ordinary, transferred
and prepared forks. The fixed source contains 1,811 files; all 61 packaged file
hashes were verified before and after the experiments. R9 was built from frozen
R8 plus this change. Later main changes, including the relative-path extension
in `9582c3f`, are outside this measurement.

The full install recorded 330.578 seconds of Job CPU (113.828 user and 216.750
kernel), 5,391 native processes and 43,264,931 page faults. No build or separate
benchmark ran in this task during the timed installation. The low-overhead
observer used 0.359 seconds of CPU over the driver run. Foreign build or runtime
activity was observed in 105 of 128 installation samples, and the quiet-start
condition was not met. The previous R8 result was 269.811 seconds; these full
runs do not isolate the change from host conditions. Maintainer scripts,
triggers and synchronization operations remain enabled.

[Measurement and evidence hashes](measurements/native-read-open-r9-2026-10-01.json)
retain exact phase timings, distribution hashes, the switch comparison, test
counts, failed pre-fix probes and corrected validation. Raw evidence is under
`artifacts/goal-install-180-20260930/`.
