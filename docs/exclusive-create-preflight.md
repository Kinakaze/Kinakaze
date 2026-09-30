# Exclusive creation without a separate link probe

`open(O_CREAT | O_EXCL | O_NOFOLLOW)` now lets the atomic create reject an
existing leaf. The former link preflight repeated native metadata opens for
new files and returned ELOOP for an existing hosted symlink. Exclusive creation
requires EEXIST in that case; ordinary O_NOFOLLOW opens still return ELOOP.
See the [Linux open contract](https://man7.org/linux/man-pages/man2/open.2.html).
The existing five native installation tests pass, including the expanded matrix
of access modes, O_NOFOLLOW, regular files, directories, FIFOs and dangling links.

A separate diagnostic unpack of core-js and core-js-pure on distribution
`5b9c117` motivated inspection of this path. Fifty snapshots of the busiest
owned process's main thread found 15 instruction pointers at NtFlushBuffersFile,
seven at NtFlushBuffersFileEx, ten at NtCreateFile and four at
NtQueryAttributesFile. These are sparse instruction-pointer observations, not
CPU percentages or reconstructed call stacks. Raw stack words are candidates
only. WPR could not start because the host refused profiling privileges.

That diagnostic unpack passed in 27.593 seconds and is excluded from timing
comparisons. The operation still performs ordinary writeback and durability
barriers. No complete-install speedup is claimed for this change yet.
Artifacts: `artifacts/apt-180-20260930/root-unpack-sample/` and
`root-exclusive-native-tests.log` in the same artifact parent directory.
