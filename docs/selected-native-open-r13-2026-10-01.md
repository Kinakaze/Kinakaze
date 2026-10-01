# Selected native file-open paths

The measured native read and write paths are now the only production choices
for eligible ordinary files. The experimental `KINAKAZE_NATIVE_READ_OPEN`,
`KINAKAZE_NATIVE_READ_RELATIVE` and `KINAKAZE_NATIVE_WRITE_OPEN` switches have
been removed. Earlier [read](native-read-open-performance-2026-10-01.md) and
[write](native-write-open-r11-2026-10-01.md) comparisons recorded the selection
evidence; their switches describe those historical frozen releases.

Writable opens now also accept simple cwd-relative names. The already resolved
absolute path goes through the same native lookup, live inode classification,
permission and verity checks, mount writer acquisition and descriptor lifetime.
Explicit directory descriptors, dot components, symlinks, special files,
unsupported flags, overlays and callers without DAC override retain the full
resolver. No metadata is reused between operations.

## Same-source comparison

Both Release builds come from `9007a9d88fbcc15a8d0a396acec978111be61511`. Only the five recorded
source/test files differ between them; generated export definitions are common.
Each of four sessions in control/selected/selected/control order recreates 512
files before timing. Every loop performs 8,192 opens and closes; write loops
also write seven bytes. All file contents are checked after the loops. Python
startup is outside the body timing. No own compilation overlaps these loops;
the host is not exclusively reserved, so these local gains do not predict
whole-install wall time.

| Loop | Control mean (ms) | Selected mean (ms) | Reduction |
| --- | ---: | ---: | ---: |
| absolute_repeated | 1263.265 | 1161.421 | 8.06% |
| absolute_varied | 1236.575 | 1226.786 | 0.79% |
| absolute_varied_write | 2536.544 | 2365.277 | 6.75% |
| relative_repeated | 2688.101 | 1516.367 | 43.59% |
| relative_varied | 2515.785 | 1685.244 | 33.01% |
| relative_varied_write | 4072.883 | 2571.133 | 36.87% |

## Validation

The selected build passed 111 focused native tests (3
declared ignored helpers/benchmarks) and 57 packaged guest
checks across ordinary, transferred and pooled fork modes. The new tests check
retained writable inode identity after replacement, renamed explicit dirfd and
cwd directories, symlink-before-dot resolution, and error behavior. Existing
tests cover permissions, readonly mounts, verity, duplication and fork/exec.
Both complete Release builds passed export validation and packaging.

An initial restore preserved older source timestamps and reused the baseline
binary. Matching VFS hashes rejected that candidate before guest or performance
measurement. The corrected build recompiled the changed inputs and verified a
distinct VFS binary; only its tests and measurements are reported here.

A new complete installation has not yet been measured for this source. The 180-second target is not established by these local loops.

[Measurements and evidence hashes](measurements/selected-native-open-r13-2026-10-01.json)
retain all four raw rows, both distribution manifests, exact source changes and
validation provenance. Full raw artifacts remain in
`artifacts/goal-install-180-20260930/r13-*`.
