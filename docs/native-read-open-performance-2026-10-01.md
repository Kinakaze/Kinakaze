# Native read-only file opening

Current production now uses the selected native paths without the experimental
read/write switches described below. See the
[route selection](selected-native-open-r13-2026-10-01.md); these measurements
and switches refer to the historical frozen release.

Ordinary read-only native files now use one retained data handle instead of
walking every path component and reopening the selected file. The same path
also accepts simple names relative to the current working directory. Live inode
metadata and credentials are checked before descriptor installation; rename,
unlink, replacement, shared offsets and descriptor flags retain their behavior.

This path applies only with native DAC override, without an overlay or an
intervening mount. Junctions, hosted links, special inodes, explicit dirfd names,
dot components, trailing slashes and unsupported flags use the existing walker.
In particular, link traversal precedes interpreting `..`. No path observations
or mutable metadata are cached across operations.

The frozen release passed 55 native regressions and 22 packaged guest checks
(11 checks with the relative path disabled and enabled). One existing native
capability diagnostic was ignored. The release packaging check passed all 29
native modules. The tests include a renamed retained directory, shared offsets,
unlink and replacement, symlink parents, FIFO fallback and junction rejection.

In six alternating runs of the same immutable release, 6,000 relative opens,
32-byte reads and closes across 200 files took a median **1,407.470 ms** with
`KINAKAZE_NATIVE_READ_RELATIVE=0`, versus **805.457 ms** enabled: **42.77%** less
guest body time. Job CPU medians were 1,468.750 and 937.500 ms. Absolute-path
control medians were 641.086 and 669.249 ms, respectively. These measurements
exclude Python startup from body time; every read checked its expected content.

This measures the relative extension against the absolute native-read path in
the same binary. It does not establish a whole-install speedup or the 180-second
installation target. Background compilers appeared in all ten host samples.
The frozen source is based on `1153b905ed99` plus the exact native-read cohort;
later changes to main are outside this release's measured source.

[Evidence, source hashes and distribution hashes](measurements/native-read-open-2026-10-01.json)
preserve every validation and measurement row. `KINAKAZE_NATIVE_READ_OPEN=0`
disables the complete path for diagnosis; both switches default to enabled.

The same frozen release then completed a fresh full **357-package** Node.js/npm
installation in **261.841 seconds**, with download measured separately at
2.022 seconds. Node, npm and dpkg audit passed; full package-file verification
passed in another 11.468 seconds. Installed names and versions exactly matched
the R11 full-install reference. Maintainer scripts, triggers and synchronization
remained enabled, and all 59 distribution hashes remained unchanged. The first
setup directory failed initialization and was excluded; a separate successful
setup supplied the fresh measured root.

The install Job used 356.578 CPU seconds (123.969 user, 232.609 kernel), created
5,275 processes and recorded 41,861,962 page faults. No build or separate
benchmark ran in this task during installation. The quiet-start condition was
not met; foreign compilers appeared in all 261 installation host samples,
with median total host CPU at 59.1%. The observer used 0.766 CPU seconds across
the complete driver. This source and host differ from the best 220.020-second
observation, so this run does not isolate installation savings. The 180-second
target remains unmet.

[Complete-install evidence](measurements/native-read-open-install-2026-10-01.json)
records all eight phases, CPU counters, frozen source and distribution hashes,
fresh-root setup results and host observations.
