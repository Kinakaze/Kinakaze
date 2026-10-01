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

Whole-probe Job CPU, including Python startup and fixture preparation, averaged
14.750 seconds for the control and
10.930 seconds for the selected route. Absolute-path write
samples vary substantially; the small absolute-path mean differences do not
establish a separate reliable improvement from removing the switches.

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

The frozen selected release completed a fresh full 357-package Node.js/npm installation in **304.284 seconds**. Downloads took 6.818 seconds separately; full installed-file verification took 134.857 seconds. Node, npm, dpkg audit and every phase passed; all 357 package versions matched R11 and all distribution hashes remained unchanged. Maintainer scripts, triggers, writeback and fsync were enabled. The 180-second target remains unmet.

The quiet-start condition was not met; 152 of 152 two-second installation samples recorded foreign CPU growth. The observer used 1.281 CPU seconds. No own compilation or other benchmark overlapped installation. Sampling cannot prove continuous host isolation. The frozen source is still the recorded base plus these five files; later main COW/PI selections are outside this measured binary.

The first install invocation was rejected by the fixture-directory check before using the root. The successful retry used the byte-identical workspace harness; all five harness hashes were checked. The rejected invocation and its earlier 600-second quiet wait are retained separately.

## Comparable installation fixture

The earlier 185.597-second observation uses the original 16,127-file seed.
Its stderr contains `Cannot get debconf version` and `apt-extracttemplates failed`:
APT's extractor did not obtain the debconf version from the base package's
versioned Provides. The current 16,128-file seed preserves the Debian extractor
behind the production compatibility adapter and completes that preconfiguration
work. The adapter and the corresponding base-package file/MD5 records account
for all four changed paths. R10 and this R13 run have empty guest stderr.

Consequently, 185.597 seconds is not a functionally equivalent baseline for this
fixture. The best verified result with working preconfiguration remains
[R10's 215.415 seconds](goal180-r10-integrated-install-2026-10-01.md).
The Node installation benchmark now rejects these explicit preconfiguration
errors even when apt exits successfully; this acceptance check was added after
the R13 measurement and the raw historical reports are retained unchanged.

[Measurements and evidence hashes](measurements/selected-native-open-r13-2026-10-01.json)
retain all four raw rows, both distribution manifests, exact source changes and
validation provenance. Full raw artifacts remain in
`artifacts/goal-install-180-20260930/r13-*`.
