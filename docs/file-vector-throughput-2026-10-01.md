# File-vector throughput and complete small-file operations

Regular-file `readv`/`writev` now share one inode pin and one open-description
position transaction across the vector. This also covers `preadv2`/`pwritev2`
with offset -1, including raw syscalls 19/20/327/328. The selected batch applies
to two or more records whose capped total averages at most 64 KiB per record.
Larger segments use the established route after the unrestricted candidate
showed worse MiB-segment shared-descriptor medians.

The operation allocates no gather buffer and makes no extra aggregate copy.
Every segment retains its native data, verity, permission, resource-limit and
completion boundaries. Partial transfers advance the shared offset by the
bytes actually moved; append uses the resulting file size. Concurrent dup
operations cannot enter between selected vector segments. The pin preserves
the original inode across descriptor close/reuse, and signals run after pins
and position locks unwind, including SIGXFSZ after a partial write.

The slower eligible-file reference is selectable only in test builds through
`KINAKAZE_TEST_FILE_VECTOR_OPT`. Production uses the measured size selection;
its compiled libc image contains no reference-switch string. Existing stream
backends for other descriptor kinds remain necessary.

Five alternating measured pairs, preceded by one warmup pair, compared the
same frozen release test executable. Bulk rows transfer 128 MiB each through
raw syscall entry points and include a seek per iteration. Buffers have gaps;
the improvement does not rely on contiguous-buffer merging. This is cached
file throughput, with no fsync in the timer, not physical-device throughput.

| 128 MiB workload | Reference MiB/s | Selected MiB/s | Throughput gain |
| --- | ---: | ---: | ---: |
| Local read, 1024 x 4 KiB | 1080.6 | 1322.9 | 22.4% |
| Local write, 1024 x 4 KiB | 863.8 | 1020.7 | 18.2% |
| Shared read, 1024 x 4 KiB | 880.3 | 1330.4 | 51.1% |
| Shared write, 1024 x 4 KiB | 680.6 | 1027.3 | 50.9% |
| Shared read, 32 x 64 KiB | 9669.6 | 12081.6 | 24.9% |
| Shared write, 32 x 64 KiB | 7408.0 | 9036.0 | 22.0% |

Small-file throughput is the priority acceptance workload. Each file is newly
created, written, closed, reopened, read and closed. Path preparation and final
byte verification are outside the timer. A confined root exercises production
native exclusive-create/read-open paths. The final isolated experiment creates
1024 files of 4 KiB, comparing ordinary scalar raw read/write with eight-segment
raw readv/writev; it runs without the preceding bulk benchmark.

| Isolated 1024-file chain | Reference median | Selected median |
| --- | ---: | ---: |
| Scalar read/write, unchanged route | 1386.718 ms | 1410.833 ms |
| Eight-segment readv/writev | 1466.891 ms | 1403.613 ms |

The vector median improves 4.3%, from 698.1 to 729.5 files/s, but individual
pairs vary and two selected observations are slower. The earlier combined
512-file median improved 21.0%; the isolated result supersedes it as the main
small-file observation. Ordinary scalar file I/O is not accelerated by this
change. Other unchanged rows also vary substantially, so their apparent gains
are not attributed to the implementation. Foreign compilation was observed
at the isolated experiment's start and end. No dedicated-host or complete
`npm i` installation speedup is established.

Both reference and selected routes passed seven new functional cases. Together
with pread, pipe-vector, native capability lifetime, positional, verity, native
open and device/Unix routing regressions, the frozen suites passed **80 tests**,
with zero failures. Eight diagnostic/helper cases were ignored in the functional
runs; the two new benchmarks were measured separately. Production release build/check also
passed. The measurements use base `98b3312` plus the recorded source hashes;
later main integrations are tracked separately, not added to these timings.
The merged graph `427f466` also passed the production release check in 23.00 s;
its incoming datagram changes are outside these frozen file-vector timings.

[Complete rows, hashes and limitations](measurements/file-vector-throughput-2026-10-01.json)
identify both frozen cohorts, native logs, the production binary, source archive
and rejected unrestricted experiment. Local raw records remain under
`artifacts/syscall-all-paths-20261001/file-vector-*`.

Reproduce with freshly frozen native test artifacts and
`tools/benchmark-futex-queues.py --case file-vector` or `--case small-files`,
passing their libc test executable, a new output file and `--rounds 5`.
