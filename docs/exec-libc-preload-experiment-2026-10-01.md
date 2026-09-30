# Native libc preparation experiment

A frozen experimental worker prepared the mandatory native libc DLL before
activating an existing exec-stock worker. It retained the inspected deny-write
file and DLL owners, checked the required native management exports, and kept
runtime API installation and Linux identity adoption after activation. ELF,
missing and symlinked libc paths retained ordinary discovery. The prototype is
preserved in the source archive; **the performance change is not merged**.

Six unprofiled runs alternated the same distribution with libc preparation
disabled/enabled/enabled/disabled/enabled/disabled. Each scenario completed 128
children and checked every exit. Native fork transfer, fork stock, exec stock
and the existing shared catalog were enabled throughout.

| Median request-to-exit time | Disabled | Enabled |
| --- | ---: | ---: |
| Pure fork control | 1.738 s | 1.806 s |
| Fork followed by exec | 3.617 s | 3.668 s |
| posix_spawn | 3.586 s | 3.648 s |
| Fork/exec Job CPU | 9.406 s | 9.672 s |
| Fork/exec Job page faults | 1,710,732 | 1,733,095 |

These samples do not establish a speedup. Competing compiler activity and
variation in the unaffected pure-fork control limit causal interpretation.
This chat did not compile or run another runtime benchmark during the timed
comparison. Host observations and every individual result are retained.

A separate diagnostic pair used 64 fork/exec operations. The enabled libc
preparation span had median 836 us. Provider-binding medians were 729 us without
preparation and 726 us with it; complete provider setup medians were 3,107 and
3,262 us. These include background workers and cannot be added into predicted
installation savings. The measured binding work remains in the active path.

The experimental Release passed three native preparation checks and 11 guest
probes in each mode. Checks include 24 repeated execs, IPC and descriptor state,
non-root real/effective/saved IDs, supplementary groups, cwd, environment and
umask, fork fallback, hosted links, robust mutex recovery and exact GNU aliases.
Both idle pools added zero Job CPU milliseconds during their one-second check.

Packaging initially exposed a separate regeneration defect: reviewed robust
pthread aliases existed only in manually extended export inputs. Regeneration
removed those aliases and exact versions. Commit `a80a71b` preserves the aliases
and reviewed versions in the generator; 23 generation tests, packaging of 29
modules/5,969 exports, and both guest alias checks passed. Version declarations
were verified against [glibc's source](https://github.com/bminor/glibc/blob/glibc-2.36/nptl/Versions).

The [measurement JSON](measurements/exec-libc-preload-2026-10-01.json) retains
source/distribution hashes, all rows, diagnostic summaries and raw-evidence
references. This experiment received no complete-install acceptance run.
The independently published integrated installation remains 220.020 seconds;
the 180-second target is still unmet.
