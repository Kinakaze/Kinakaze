# Native rename experiment, 2026-10-01

The prototype is rejected pending a correctness fix. Its ordinary native rename path acquired one private DELETE-capable source file and one target parent directory, then reused the existing relative NT rename helper. It aimed to avoid the current preliminary source/destination queries and subsequent source open. No production package, guest benchmark or complete install was run for this prototype, and no speedup is claimed.

The isolated release native suite built successfully and reported **69 passed, 1 failed, 1 existing diagnostic ignored**. The eight new cases contributed seven passes and one failure. The failure is `same_path_and_same_inode_hardlink_rename_preserve_linux_names`: same-path rename succeeded, but replacing a different hardlink to the same inode removed the source name. The NT request itself returned success before the assertion failed. This contradicts the [POSIX rename specification](https://pubs.opengroup.org/onlinepubs/9799919799/functions/rename.html), which requires success with both names preserved for this case.

Passing new cases cover independent source and target-parent rename/replacement after selection, no-replace collisions, replacement of an open target, hosted links and FIFO records without following or reclassification, dot/trailing-slash and ancestor-link fallbacks, escaped names under a guest namespace, final/ancestor native junction rejection, and the complete selector with mount writers. The 62 existing native create/read, metadata/lifetime, verity, namespace, lookup and open-description regressions passed. These results do not establish that every guest permission or mount rule is correct.

The exact tested source was detached `4806716` plus a 32-file delta containing the four implementation files and baseline formatting. The complete delta is frozen under `artifacts/apt-180-20260930/r15-native-rename-prototype-sources.zip`; its manifest and all hashes are retained in [the measurement](measurements/native-rename-experiment-2026-10-01.json). The [four-file experimental patch](experiments/native-rename-prototype-2026-10-01.patch) saves the implementation and regression in Git for review and continuation. It is an unapplied experiment, not part of the product build. The complete failed test record is embedded in the measurement; the failed prototype is not described as an accepted distribution.

Before continuing, compare the existing Win32 helper against the same hardlink regression; this experiment did not verify baseline behavior. Any candidate must preserve same-inode no-op semantics, no-replace atomicity and inode identity before performance measurement. Handling only no-replace calls would leave the ordinary replacement workload unoptimized. A target identity check must account for replacement races and its own syscall cost; simply treating the successful NT request as a Linux rename is insufficient. Avoid relying on a persistent pathname or metadata cache.

The original 180-second goal remains open. The latest documented accepted integrated observation is 215.415 seconds; the last complete native-create candidate took 253.647 seconds with every installation and validation phase passing. Neither observation proves that this rejected rename design improves installation performance. The user requested a quick wrap-up and push, so this checkpoint records the tested failure without starting a new production build or installation.


## Native no-replace follow-up

A direct native API probe tested same-path, different hardlink/same-inode,
and different-inode targets with flags 0, 1, 2, 3, 0x41 and 0x43. The latter
matches the current Win32 helper's flags. All six flag combinations succeeded
and removed the source name for the different-hardlink/same-inode case.
No-replace rejected a different-inode target with native error 183, but did
not reject another name of the same inode. All same-path cases retained the
name. All 18 observations and script hashes are in the measurement JSON.

This rules out the proposed shortcut of trying no-replace first and falling
back only on a collision: a successful native call can already have removed
a Linux name that should remain. The probe exercises the matching Windows
API directly, not a rebuilt Rust helper or a packaged guest. It does not claim
to fix the baseline or establish a performance gain. Production rename code
remains unchanged, and the rejected prototype remains unapplied.
