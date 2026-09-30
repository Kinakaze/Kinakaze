# Integrated native-read installation

The frozen `22df00b` release completed a fresh 357-package Node.js/npm
installation in **204.887 seconds**. Download took
2.419 seconds separately. Maintainer scripts,
triggers and synchronization remained enabled. Node and npm smoke checks,
empty dpkg audit, and full file verification all passed; verification took
another 10.634 seconds. Installed package versions
exactly match the preceding 220.020-second integrated reference. All 59 frozen
distribution hashes were rechecked unchanged.

The source combines the retained native-read implementation with fork/exec
preparation, shared provider catalogs, retained process identity capabilities,
exclusive-create preflight and descriptor metadata capabilities. It also
includes the main-branch shared pthread mutex implementation. The export
generator now emits each native target once while retaining all public and
versioned aliases; all preexisting export lines remain present.

The installation Job used 288.078 CPU seconds
(99.516 user and 188.562 kernel),
created 5,275 processes and recorded 41,929,290
page faults. The driver required 20 continuous observed quiet seconds before
starting. Two earlier quiet checks expired without starting an installation;
the fresh root remained unused. During the completed run, the sampler excluded
members of this install's Windows Job, including descendants whose native
parent had exited.
Of 203 installation samples, 58
contained a foreign compiler and
56 showed at least 0.05 CPU seconds
of competitor growth between consecutive samples. Median host CPU was
30.8%. The monitor and quiet check used
0.500 CPU seconds. All measured
harness files retained their original hashes.

One zero-CPU runtime worker appeared outside its Job snapshot, with a birth
time just after the sample timestamp. An own child can start between the
membership query and process enumeration, so this observation cannot establish
foreign runtime activity.

The integrated VFS source passed 93 native tests with one existing capability
diagnostic ignored. Sixteen packaged guest probes and 25 export-generator
tests passed. The final release passed validation of 29 native modules and
5,971 guest exports. The idle pool consumed zero CPU milliseconds during the
one-second observation. Raw evidence is under
`artifacts/apt-180-20260930/root-read-open-*` and
`artifacts/node-install-20260930/root-read-open-install/`.

The requested sub-180-second target remains unmet.
Historical source and host conditions differ, so this is an integrated
observation rather than isolated attribution to one change. A quiet start
does not ensure an otherwise idle host throughout the run. The sampler may
miss short-lived foreign work and does not equate worker presence with CPU
activity.

[Measurements and evidence hashes](measurements/integrated-native-read-install-2026-10-01.json)
retain the complete result and regression records.
