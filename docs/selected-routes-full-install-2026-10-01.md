# Complete installation with selected production routes

The selected routes built from `98b3312947b5e43d0acc83b3ce6476550765417b` completed a fresh
357-package Node/npm installation with working debconf preconfiguration in
**208.099 seconds**. Download took
2.310 seconds and complete installed-file verification took
12.067 seconds, measured separately. The sub-180-second target remains unmet. The best verified observation with working preconfiguration is 208.099 seconds.

The historical 185.597-second result is not accepted under the current check:
its legacy seed produced `Cannot get debconf version` and `apt-extracttemplates
failed`. These errors let APT continue while preconfiguration failed. This run
uses the corrected 16,128-file seed and the updated benchmark assertion. All
eight phases pass, all 357 names and versions match R10, and preconfiguration
errors are absent. Raw historical reports remain unchanged.

## Retained routes and scope

Production retains [compact descriptor-link storage](fd-link-layout-selection-2026-10-01.md),
[selected native file-open paths](selected-native-open-r13-2026-10-01.md), and
[coalesced private COW copying](fork-cow-coalescing-2026-10-01.md).
Legacy descriptor layout and experimental native-open switches are removed.
The release also contains the validated process/futex changes through the
recorded source commit. Component comparisons justify route selection; this
full run measures their combination and does not isolate any one change.

The corrected seed preserves the Debian template extractor behind the production
compatibility adapter, with corresponding base-package file and MD5 records.
Its manifest SHA-256 is
`b4aa01f0ca1e4f11a6156ccebcc2527331e80f4cbe7631cd50e131c235182e83`.
The seed and initial package status were recorded before installation. Scripts,
triggers and synchronization remain enabled. Update, download, plan, installation,
Node/npm smoke checks, empty audit and complete installed-file verification pass.

| Install Job metric | Result |
| --- | ---: |
| Installation | 208.099 s |
| Job CPU | 272.203 s |
| User / kernel CPU | 99.281 / 172.922 s |
| Native processes | 5,391 |
| Page faults | 35,464,079 |
| Samples with a compiler present | 101/207 |
| Samples with foreign runtime workers | 1/207 |
| Observed foreign CPU growth | 211.062 s |
| Median host CPU | 35.70% |

The earlier functionally comparable [R10 run](goal180-r10-integrated-install-2026-10-01.md)
took 215.415 seconds. Source and host conditions differ,
so this comparison does not establish an isolated speedup or timing variance.
No own compilation, setup or separate benchmark overlapped installation. No
quiet-start condition is claimed; sampling can miss short-lived competitors.

## Validation and provenance

The integrated runtime passed 99 native tests (two declared ignored diagnostics),
28 packaged guest probes, and export validation for 29 modules / 5,973 exports.
Probes cover fork/exec pools, concurrent vfork, exit/reaping, descriptor paths and
transfer, credentials, namespaces, permissions, native opens, sparse fork stacks,
PI requeue, signal restart and robust pthread mutexes. Idle worker inventory and
CPU checks passed, and failure-only fork diagnostics remained empty. The earlier
one-off selected-image `EAGAIN` observation remains recorded in the layout report;
no cause or fix is claimed here.

The first integration package passed the same 28 probes but had unverified static
entry provenance: the mutable entry directory supplied different `init.exe` and
`worker.exe` hashes. A new package preserved the other 57 files and copied the
two entries from the frozen, previously validated distribution. All host entry
source and dependency paths are unchanged since that build. The corrected
package passed all 28 probes again; all 59 hashes remained unchanged through the
full installation. The first package and both reports are preserved.

An initial run of the corrected package on the legacy seed recorded
186.504 seconds and passed the old eight phase checks, but
is rejected for preconfiguration errors. The workspace harness also changed
during that run, so its driver's integrity check failed. It is not an accepted
performance result. The accepted rerun executes byte-identical frozen copies of
all five benchmark modules, including the new preconfiguration assertion. The
benchmark retains its original filename for the existing disposable-root check;
the source bytes and imported modules come from the frozen harness. Its hashes
are checked after completion.

[Measurements and evidence hashes](measurements/selected-routes-full-install-2026-10-01.json)
retain the exact source/images, full phase metrics, both entry packages, rejected
legacy run, corrected seed, frozen harness and host observations.
