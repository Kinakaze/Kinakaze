# Selected fork and VFS installation validation

Comparison update: the historical 185.597-second result uses the older seed
with failed debconf template extraction. It is not a like-for-like baseline
for the current working-preconfiguration fixture; see the
[seed comparison](selected-native-open-r13-2026-10-01.md#comparable-installation-fixture).

The frozen production source `2c66b62c92b46048680811fc8e725c828ad2d028` completed a fresh installation
of the same 357 Node.js/npm packages in **292.443 seconds**.
All eight phases passed: update, download, plan, installation, Node and npm smoke
checks, empty dpkg audit and full installed-file verification. Package names and
versions exactly match the previous reference. Downloads took
2.564 seconds and verification took
21.727 seconds, both reported separately.

This production build combines selected absolute/relative native read and write
opens, compact descriptor-link headers, the selected PI metadata path and
[coalesced COW copy ranges](fork-cow-coalescing-2026-10-01.md), with the existing
fork/exec pools and native catalog. Scripts, triggers, writeback and fsync remain
enabled. The guest root was independently installed from the original immutable
seed. Its initial package status and seed manifest hashes are recorded.

The build passed validation of 29 native modules and 5,973 guest exports.
Thirteen packaged guest checks passed, including protected and sparse stacks,
nested forks, native transfer and fallback, pooled fork/exec, vfork, provider
metadata, native lookup/read cache and pthread/futex behavior. The added 16 MiB
private mapping check crosses query batch boundaries, preserves clean gaps,
forks twice, checks parent/child isolation and confirms the backing file stays
unchanged. All 59 distribution hashes matched before and after installation.

| Install Job metric | Result |
| --- | ---: |
| Total CPU | 365.141 s |
| User CPU | 125.406 s |
| Kernel CPU | 239.734 s |
| Processes | 5,275 |
| Page faults | 34,258,628 |
| Other I/O operations | 9,608,815 |

The 20-second observed quiet-start prerequisite was
not satisfied; this run started under observed load.
During installation, 288 of 288 samples
included a foreign compiler and 283 included
a foreign runtime process. Consecutive-identity competitor CPU growth totaled
736.891 seconds; median host CPU was
51.50%. Sampling can miss short-lived activity.
Own compilation, setup and separate probes completed before timed installation.
All five benchmark tool hashes remained unchanged.

The 180-second target remains unmet in this observation.
The previous best verified installation was 185.597 seconds. Component route
comparisons justify their selection; this combined run does not isolate each
change or establish timing variance across different host/storage states.

Every packaged DLL and import library came from this Cargo invocation; static
entry executables came from the matching source with static std linkage. The
earlier R17 packaging attempt omitted import libraries and failed before
publication. R18 records the complete graph and passed packaging/export checks.

[Complete measurements and provenance](measurements/integrated-selected-cow-install-2026-10-01.json)
retain source, image and evidence hashes, all phase metrics, guest checks and host
observations. Raw artifacts are under `artifacts/apt-180-20260930/*r18-cow*`.
