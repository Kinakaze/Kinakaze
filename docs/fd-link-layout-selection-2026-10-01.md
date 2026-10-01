# Retain the compact descriptor-link layout

Production now has one descriptor-link layout: contiguous 16-byte headers
and separate 1,024-byte targets. The legacy interleaved implementation and
`KINAKAZE_COMPACT_FD_LINKS` switch have been removed. The retained header
stride must equal 16, so an incompatible legacy mapping is rejected before
slot access. Compact layout offsets, capacity, magic and section names are
unchanged from the previous compact implementation.

The selection is supported by lower fork/exec latency and fewer page faults.
It is **not** a claim that complete APT installation is faster: the two full
observations below ran under different foreign loads, and the compact one
had worse wall time and Job CPU.

## Interleaved fork/exec/wait comparison

One unchanged 59-file distribution ran eight sessions in
legacy/compact/compact/legacy/compact/legacy/legacy/compact order. Each session
performed 128 fork/exec/wait operations, checking the child identity, parent,
group, session and exact exit status every time. Medians use four sessions
per layout. Guest timings exclude Python startup; Job CPU includes the
whole command. All 1,024 child cycles passed, and image hashes stayed equal.

| Median per 128 operations | Legacy | Compact |
| --- | ---: | ---: |
| Guest fork/exec/wait | 3.616547 s | 3.509863 s |
| Job CPU | 8.007812 s | 8.007812 s |
| Job page faults | 1,628,313.5 | 1,364,771.5 |

Compact median fork/exec latency was 2.95% lower, page faults were
16.18% lower, and median Job CPU was equal. This bounded comparison
and the earlier [cold-view scan comparison](fd-link-layout-2026-10-01.md)
justify retaining compact storage. They do not predict an APT wall-time gain.

## Complete installation observations

Both runs used separate fresh roots from the original seed, the same 59-file
distribution, the same five benchmark tools, and identical 357 package names
and versions. Update, download, plan, installation, Node/npm smoke checks,
empty audit and installed-file verification all passed. Scripts, triggers
and synchronization remained enabled. Downloads and verification are separate
from the installation metric.

| Measurement | Legacy | Compact |
| --- | ---: | ---: |
| Complete install | 209.602 s | 216.739 s |
| Download | 1.748 s | 2.314 s |
| Full verification | 12.371 s | 13.418 s |
| Job CPU | 261.062 s | 271.984 s |
| Job page faults | 39,676,216 | 34,219,379 |
| Native processes | 5,275 | 5,275 |
| Samples with a compiler | 137/208 | 216/216 |
| Samples with foreign runtime workers | 0 | 11 |
| Observed foreign CPU growth | 132.938 s | 259.047 s |

Compact incurred 13.75% fewer page faults. Its initial bounded
300-second attempt never reached the required 60 quiet seconds and exited
without starting installation. Its unused root was then tested under observed
load, with no quiet-start claim. The changed start condition and heavier
foreign activity prevent an isolated conclusion about full-install wall time.
They do not establish how much of the difference came from interference.

## Selected implementation validation

After removing the legacy branch, 40 native process-table tests and all 23
packaged guest probes passed. The probes include fork/exec pools, concurrent
vfork, exit/reap, proc descriptor paths, namespaces, Unix descriptor transfer,
filesystem boundaries and native open/create behavior. Idle worker inventory
and CPU checks passed. All 59 selected distribution hashes were unchanged;
export checks covered 29 modules and 5,973 exports.

The first selected-image suite failed once in `ProcessExitBoundaryProbe`
when `fork` returned `EAGAIN` during concurrent reaping. Fresh-root setup was
running concurrently, but a causal connection is unproven. Without changing
source, images or assertions, an isolated traced probe, the complete 23-probe
suite with failure-only diagnostics, four further selected-image exit probes,
and four old-image exit probes all passed. No cause was identified and no
fix is claimed for that initial observation. Its failed report is retained.

The selected release was built from `fc4279f42a09128f63a6457c219bde97c67ae6e1`
plus the two recorded source files. Static init/worker entry points retain
the unchanged compact table protocol. The earlier dual-layout distribution
and its exact source snapshot remain as historical comparison artifacts;
production contains no legacy-layout path.

The rebuilt compact-only release completed a fresh full installation in
**209.810 seconds**, with download at 2.957
seconds and complete verification at 10.794 seconds separately.
Every phase passed, all 357 installed package versions matched, and all 59
image hashes stayed unchanged. This run used observed host load, with
202/208 compiler samples
and 0 samples with foreign runtime
workers. No quiet-start condition is claimed.

The best verified complete-install time is
**185.597 seconds**.
The sub-180-second target remains unmet.
[Full measurements and evidence hashes](measurements/fd-link-layout-selection-2026-10-01.json)
preserve the initial failed quiet attempt, both full comparisons, all eight
fork sessions, exact source/image hashes, and final-route validation.
