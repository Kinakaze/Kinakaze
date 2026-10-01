# Integrated native write/create installation

The validated frozen release completed a fresh 357-package Node.js/npm
installation in **206.074 seconds**, with downloads
timed separately at 1.766 seconds. Every phase
passed: update, download, plan, complete installation, Node and npm smoke
checks, empty dpkg audit and installed-file verification. Package names and
versions exactly match the previous 204.887-second reference. The full
verification phase completed in 116.269 seconds and is
reported separately from the install metric.

This run combines native read-open, native write-open and exclusive-create
paths with the existing fork/exec pools, shared provider catalog and
descriptor/process capabilities. It also contains the selected-parent
permission repair. Maintainer scripts, triggers, writeback and fsync remained
enabled. All 59 frozen distribution hashes were unchanged after the run.

| Measurement | Previous integrated reference | This release |
| --- | ---: | ---: |
| Complete install | 204.887 s | 206.074 s |
| Job CPU | 288.078 s | 258.766 s |
| User CPU | 99.516 s | 91.641 s |
| Kernel CPU | 188.562 s | 167.125 s |
| Native processes | 5,275 | 5,275 |
| Page faults | 41,929,290 | 39,713,121 |
| Other I/O operations | 12,294,013 | 9,593,968 |

Job CPU was 10.18% lower and other I/O operation count was
21.96% lower, but wall time did not improve. These are integrated
observations under different host conditions, not isolated causal attribution
to one change. This first run did not improve the 204.887-second reference; the later
repeat below is the new best. The sub-180-second target remains unmet.

The driver required 20 continuous observed quiet seconds before launch and
performed no simultaneous build, setup or separate benchmark from this task.
Foreign compilation started during installation: 116
of 204 samples contained a compiler, and
114 samples showed competitor CPU
growth of at least 0.05 seconds. The consecutive-identity CPU sum was
122.016 seconds; short-lived processes
can be missed. No foreign runtime worker was observed. Median host CPU was
34.75%. Job membership is checked again after the
initial snapshot to exclude newly born own children. Observation and quiet
checking used 0.672 CPU seconds.
All five benchmark harness hashes match the previous reference and remained
unchanged during this run.

The exact source is commit `2bfb28fd3efc383fe5559701a60e32226b036581` before its permission
fix was rebased onto later main, equivalently base `4806716` plus the five
recorded source files. Its export definitions differ from compile-time input
only in declaration ordering; complete nonempty line multisets were proven
equal. The same frozen release passed 22 packaged guest checks, 25 export
generator tests and validation of 29 modules with 5,973 guest exports.
Later unrelated PI-requeue changes in main are not part of this frozen result.

The large verification wall time and unchanged install wall time warrant
separating storage/host waits from the reduced CPU work before claiming a
further speedup. The existing
[VFS diagnostic report](node-install-vfs-diagnostics-2026-10-01.md)
identifies ordinary opens, synchronization and rename as remaining costs,
but its instrumentation and source differ from this acceptance run.

[Complete measurements and evidence hashes](measurements/integrated-native-write-create-install-2026-10-01.json)
preserve every phase, source/binary provenance, host observations and the
comparison. The earlier
[204.887-second result](integrated-native-read-install-2026-10-01.md)
is the historical reference for these comparisons.


## Same-binary fresh-root repeat

The same unchanged release completed another fresh installation in
**185.597 seconds**, the new best verified result.
Downloads took 2.656 seconds separately; Node/npm,
audit and full file verification all passed. Verification took
10.601 seconds. All 357 package names/versions matched,
and all 59 hashes were unchanged. Job CPU was 229.516
seconds (83.484 user, 146.031 kernel),
with 5,275 processes, 39,713,856 page faults and
9,595,454 other I/O operations.

The quiet-start prerequisite was satisfied, but the entire run was not idle:
26 of 184 samples included a foreign
compiler; no foreign runtime worker was observed. Consecutive-identity
competitor CPU growth totaled 29.734
seconds. Median host CPU was 27.3% and observation
used 0.500 CPU seconds. Fewer observed
competitors coincide with the lower wall/CPU times; this does not identify
all causes of the difference. The remaining target gap is
5.597 seconds.

A separate sampled rerun of `dpkg --verify` on the first installed tree passed
in 9.783 seconds using the same binary. The first run's 116.269-second verify
delay did not reproduce. Seventeen native snapshots were collected; raw stack
candidates are not reconstructed call stacks or CPU percentages. This is a
diagnostic rerun, not an installation measurement. Its evidence is retained
without replacing the original long verification result.
