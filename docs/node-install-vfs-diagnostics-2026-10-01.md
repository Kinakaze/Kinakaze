# Full-install VFS operation diagnostics

The frozen R8 source was rebuilt with 51 aggregate operation counters. Each
native process writes a separate mapped record, with a process identity guard
for copied fork state. Seven packaged guest probes passed, including long
arguments, fork/exec, permissions, namespace paths and native reads; 69 process
records passed counter consistency checks. The production R8 distribution was
unchanged.

The diagnostic fresh installation configured all 357 packages and passed
Node/npm, empty audit and complete file verification. Its guest command took
227.009 seconds and 320.547 Job CPU seconds. This is an instrumented observation,
not a new acceptance measurement or evidence of a 180-second result. The host
conditions were not isolated. The published uninstrumented R8 installation
remains 269.811 seconds.

| Operation | Completed calls | Inclusive elapsed total |
| --- | ---: | ---: |
| write | 46,913 | 49.647 s |
| descriptor synchronization | 30,483 | 32.294 s |
| read | 99,871 | 29.896 s |
| openat | 91,487 | 25.845 s |
| sync_file_range | 47,960 | 15.112 s |
| rename | 59,487 | 12.607 s |
| private object open | 247,443 | 10.227 s |
| VFS state restoration | 5,370 | 7.846 s |
| private object reopen | 171,539 | 7.379 s |
| EA query and decode | 489,950 | 1.701 s |
| named inode lock acquisition | 114,172 | 1.217 s |

Nested calls and concurrent processes overlap. These totals cannot be added
into wall time or treated as CPU usage or predicted savings. In particular,
47.260 seconds of write time belongs to fork children without a new argv
publication; it can include blocking while a consumer does other work.
The state restoration entry is shared by fork and exec handoff decoding.

The dpkg group accounts for 83,118 opens and 23.344 seconds of open duration,
30,478 descriptor synchronizations and 32.278 seconds, and all 47,960 range
writeback calls. EA reads total only 1.701 seconds across the entire install;
continuing to optimize their decoding alone cannot explain the remaining gap.
The next candidate should reduce repeated native opens and observations in the
ordinary file-open path. Durability barriers remain enabled. The existing range
writeback uses the data-only native flag whose semantics are documented by
[Microsoft](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntflushbuffersfileex).

All 5,371 operation-record owners were matched to unique native process
lifetimes using PID, creation time and exit time. The census reported no errors.
Whole-lifetime CPU includes native preparation, so its group totals are kept
separate from operation durations. The controller, census, local repository
and counter reader consumed 6.531 CPU seconds altogether. Reading and recording
the counter files happened outside the guest command timer and is reported
separately in the measurement JSON; it is additional diagnostic overhead.

The [measurement JSON](measurements/node-install-vfs-2026-10-01.json) retains all
operation and program totals, lifetime groups, limitations, and SHA-256 evidence
references. Raw records, diagnostic source, decoder and scripts are under
`artifacts/goal-install-180-20260930/`; the actual distribution has its own
`r8-vfs-dist.json` manifest and passed all 61 post-run file hash checks.
