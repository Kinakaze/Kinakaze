# Private read capabilities and retained inode mutexes

Native descriptor operations retain one same-access duplicate per local open
description. Dup aliases share that capability; final close removes the registry
entry while an in-flight pin may finish against its original inode. Overlay
operations keep their existing per-operation pins across copy-up. Private cache
handles are not serialized; fork/exec children rebuild them lazily.

A read-only native pin also captures a private read cache under the descriptor
table guard. Each request checks out an independently opened asynchronous
reader. Concurrent positioned reads use independent native requests and status
blocks. An idle reader retains its inode identity and named mutex capability;
each read still acquires that same inode transaction and queries persistent
verity state. No permission, verity answer, file bytes or logical size is cached.
Imported write-only descriptors cannot gain read access by reopening under host
privileges. Native EOF bounds ordinary reads while the lock prevents a first
hidden-tail append until completion.

Mutex capabilities retire before their private inode pins. A borrowed lock guard
cannot move between threads; release occurs on its acquiring thread. The common
wait path retains recursive acquisition, interruptible contention and recovery
from an abandoned owner. These semantics follow the
[Windows mutex contract](https://learn.microsoft.com/en-us/windows/win32/sync/mutex-objects).
Final private-handle retirement occurs after releasing the descriptor-table
guard, preventing lock inversions while an outstanding cache operation finishes.

`KINAKAZE_NATIVE_PIN=0` disables duplicate reuse.
`KINAKAZE_READ_MUTEX=0` keeps the ordinary per-operation named mutex open.
Both optimizations are enabled by default. Optional mutex-open failure falls
back to the established ordinary lock acquisition.

The round-nine integrated graph built 29 native modules and passed 115 focused
native tests, with one existing diagnostic ignored. All seven packaged probes
passed. New tests check fresh named openers against a retained capability,
recursive guards and a terminated native owner. Existing read-cache tests cover
concurrent positioned requests, later verity enable/corruption, unlinked inodes,
close/descriptor reuse and imported write-only access.

Three alternating same-binary trials switched only KINAKAZE_READ_MUTEX between
zero and one. All assertions passed. Median results:

| Guest measurement | Ordinary mutex open | Retained capability |
| --- | ---: | ---: |
| 10,000 read-only operations | 72.341 ms | 44.054 ms |
| 10,000 read/write operations | 51.867 ms | 50.107 ms |
| 20,000 present stat queries | 551.924 ms | 544.561 ms |
| 2,000 repeated chmod/chown pairs | 41.598 ms | 41.774 ms |
| 128 repeated parent forks | 3,784.914 ms | 3,766.914 ms |

The read-only median reduction is 39.10%. Other rows mostly show variation in
unchanged paths; the account-lookup median was slower with retention enabled.
This microbenchmark does not establish an equivalent installation speedup.
The binary contains other previously integrated VFS/loader changes. Its exact
hashes and fixed source graph identify the experiment; they do not establish
exclusive attribution to the isolated cache commit.

Evidence in `artifacts/apt-180-20260930/`:

- `r9-retained-mutex-sources.zip` and the matching SHA-256 manifest.
- `native-r9-retained-mutex.json`.
- `guest-r9-retained-mutex/results.json`.
- `read-mutex-paired-r9/report.json`.
- `compare-read-mutex.py`.

The independent fresh-root round-nine installation passed all 357 packages,
Node/npm, empty dpkg audit and empty complete file verification. Installation
took 369.063 s, with 353/10 s unpack/configure, 353.938 s job CPU, 5,256 native
processes and 41,579,297 page faults. Verification took 20.828 s. The initial
sparse observation found no compiler/runtime processes, but later observations
included considerable compiler activity. This run does not establish an
end-to-end speedup and does not improve the earlier validated 302.849 s result.
See `node-r9-retained-mutex/report.json` and `node-r9-host.jsonl`.
The 180-second end-to-end target remains unachieved.

The isolated cache commit was built separately on a committed native-fork-pool
base, without the other uncommitted metadata/query changes. It built all 29
native modules and passed 54 focused native tests (one existing diagnostic
ignored). Nine packaged probes passed with explicit handle transfer and native
prewarming enabled, including transfer, fallback, pooled fork, concurrent vfork,
read-cache lifetime and nested sparse stack cases. Four ordinary-path probes
also passed with both optional fork modes disabled. Results are recorded in
`native-read-cache-commit.json`, `guest-read-cache-commit-pool/results.json` and
`guest-read-cache-commit-ordinary/results.json`.
