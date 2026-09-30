# Native fork pool creation and manager locking

Refilling the native fork pool used to hold the manager and pool-state mutexes
through CreateProcess and Job assignment. Other process requests therefore
waited for background process creation. The refill loop also waited up to
10 ms before creating the next missing slot.

Creation now uses a separate gate shared with shutdown. The manager and state
locks are released while Windows creates and assigns the child. Shutdown waits
for that assignment before allowing init to exit. Publication checks the
current template generation and stopping state; rejected children remain owned
and are terminated. An Arc retains the module pins across creation, including
when a concurrent request replaces the template. Missing slots are replenished
on the next iteration without the extra polling delay.

Six native tests and 24 guest checks passed. Native checks include actual child
termination after template replacement or shutdown, exact delivery cancellation,
parent death, commit, and the creation gate. Guest checks cover ordinary,
transferred and pooled forks; the pooled run observed 39 real pool hits.

Only init.exe differs between the two measured distributions; the other 60
files have identical hashes. Both enable KINAKAZE_FORK_TRANSFER and
KINAKAZE_FORK_POOL. The baseline/candidate/candidate/baseline results were:

| Workload | Baseline 1 | Candidate 1 | Candidate 2 | Baseline 2 |
| --- | ---: | ---: | ---: | ---: |
| 128 fork/wait pairs | 2.040 s | 1.846 s | 1.742 s | 2.087 s |
| 100 external commands | 4.839 s | 4.374 s | 4.300 s | 4.848 s |
| 512-file dpkg unpack | 1.957 s | 1.848 s | 1.752 s | 1.965 s |
| dpkg configure, 8 script children | 0.523 s | 0.470 s | 0.458 s | 0.580 s |

Every run checked exits, installed bytes and package status, retaining ordinary
durability operations. No foreign compiler/worker was observed at the start
of the four runs; one appeared in the final post-run snapshot. These are local
comparisons, not proof that the full installation meets the 180-second target.
