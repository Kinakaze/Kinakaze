# Integrated Node.js/npm installation

The integrated frozen Release distribution completed the real cached Debian
installation in **288.959 seconds**. All 357 newly installed packages configured
successfully, Node.js 18.20.4 and npm 9.2.0 ran, and both dpkg audit and complete
file verification produced empty output. The 180-second goal remains unmet by
108.959 seconds.

The source baseline was `794deb9`, plus 39 frozen integration files covering
the shared native catalog, process-liveness capabilities, loader metadata and
VFS operations. Native fork transfer, fork preparation and exec preparation were
enabled together. Maintainer scripts, triggers and ordinary durability operations
remained enabled. Download was measured separately against a local hashed
repository. This is one integrated observation, not a controlled estimate of
any individual optimization's benefit.

| Measurement | Result |
| --- | ---: |
| Package download | 2.035 s |
| Installation | 288.959 s |
| Installation Job CPU | 393.453 s |
| Job user / kernel CPU | 135.391 / 258.063 s |
| Native processes during installation | 5,275 |
| Page faults | 41,783,958 |
| dpkg unpack / configuration | 279 / 6 s |
| Complete file verification | 16.277 s |

The dpkg stage timestamps have one-second resolution. The transaction uses
request-to-exit wall time. This chat performed no compilation or separate runtime
benchmark during the timed install. Sparse host observations include competing
compiler jobs; they do not establish an idle-host result or quantify contention.
The first root setup failed with host access denied before publication. Setup
of a second independent root succeeded; only that complete fresh root was used.

The same frozen integration passed 151 kernel/VFS/linker native checks and 137
service/protocol/manager checks, with three existing fixture diagnostics ignored.
Ten packaged guest probes passed, covering native read and metadata paths,
descriptor transfer, concurrent vfork, ordinary fallback, sparse write-watch,
allocation and prepared exec. Packaging checked 29 modules and 5,963 exports.
The 14 VFS files committed as `0403df9` matched the tested source hashes.

A subsequent **diagnostic** 64-child fork/exec/wait workload passed all identity
and exit checks. It is not an installation timing. Its loader spans showed
median registry restoration of 4,249 us, facade restoration of 25 us and link
of 7,068 us. Startup spans showed provider setup of 4,224 us and runtime DLL
opening of 7,309 us; these include background preparation workers. Nested and
concurrent spans cannot be added into predicted installation savings. Inspection
confirmed that handoff ownership and exec readiness already use events or
condition variables. The earlier approximately 16-ms phase medians alone do
not establish timer polling as their cause. Further work should target remaining
registry reconstruction and image startup using the retained diagnostic.

The [measurement JSON](measurements/apt-180-integrated-2026-10-01.json) retains
distribution/source hashes, every phase and SHA-256 references to native tests,
guest tests, source archive, install report, setup logs and the diagnostic.
Raw evidence remains under `artifacts/apt-180-20260930/`.
