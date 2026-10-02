# Ownership descriptor reuse

`fchown` can inspect a pinned native descriptor before opening an independent
metadata handle. The shortcut applies only to complete regular-file inode
records when the caller is authorized and ownership, set-ID bits and
`security.capability` require no changes. The inode mutex covers all record and
capability queries. Pending metadata queries retain request-specific completion
and cancellation. Changes and restricted descriptors keep the existing update
path; overlay descriptors keep their copy-up path. Native mount writer checks
still reject read-only mounts.

New writable native files request optional read-attribute/read-EA rights.
Access denial retries the original creation rights. No guest data-read access
is added to a write-only descriptor.

Nineteen focused native tests pass: six ownership tests, ten native creation
tests and three shared-I/O tests. Coverage includes unchanged-owner privilege
clearing, empty capabilities, authorization, restricted handles, incomplete
records, read-only mounts, descriptor flags, rename/unlink lifetime, offsets,
inherited ACLs and cancellation isolation. One unrelated lifecycle benchmark
remains intentionally ignored.

The full apt installation target of 100 seconds is still unmet. The preceding
R5 same-distribution core-js diagnostic measured 6,254 ownership metadata opens
taking 5.794 seconds with asynchronous writeback and 0.715 seconds without it.
These are diagnostic durations, not installation speedup claims. The initial
release build was stopped to finish and publish at the user's request, then
completed when the active performance goal continued.

The guest probe now explicitly checks the ABI's rejection of capability writes;
the native tests seed internal capabilities to verify their removal. Ownership
and writeback guest probes pass with asynchronous writeback both enabled and
disabled. A current-host comparison measures R5 at 28.188 seconds and R6 at
28.810 seconds for the two core-js packages. Reopens under `fchown` fall from
6,254 to four, but its total elapsed time remains 5.396 seconds. Descriptor
reuse alone therefore has no demonstrated unpack speedup.

Native overlap experiments distinguish 128-byte files from 4-MiB files: small
file queries wait roughly 0.8 ms during a roughly 0.9-ms data flush even through
a separate open. Large-file queries complete in about 0.01 ms during a 1-ms
flush. EA updates and reopens wait in both cases. These measurements motivate
scheduling asynchronous writeback after a short metadata-completion window.

With the same R7 distribution, a 2-ms queue window reduces core-js unpack from
28.142 seconds to 24.491 seconds compared with zero delay. Both runs perform
6,250 data flushes and 6,265 full file flushes. Full-fsync time remains about
8.7 seconds. Ownership time drops from 5.363 seconds to 0.305 seconds. These
remain focused diagnostics; the complete 357-package acceptance result is
tracked in `docs/apt-100-writeback-2026-10-01.md`.
