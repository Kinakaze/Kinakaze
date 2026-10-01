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
These are diagnostic durations, not installation speedup claims. The new guest
probe and full performance comparison have not run; the release build was
stopped to finish and publish the reviewed change at the user's request.
