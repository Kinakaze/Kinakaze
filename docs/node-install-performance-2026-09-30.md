# Node.js / npm installation performance, 2026-09-30

This investigation uses the real cached Debian `nodejs` and `npm` packages,
including all 357 newly installed dependencies. `tools/benchmark-node-install.py`
serves the original archives from a loopback repository, downloads them in a
separate phase, then times `apt-get --no-download install`. Maintainer scripts,
triggers, fsync and package contents are unchanged.

Each full installation gets a fresh root from the same immutable rootfs manifest
through `worker setup`. Copying writable root trees with hard links is unsuitable:
it would share inodes between trials and lose the hosted symlink/EA isolation.
The seed includes `Multi-Arch: allowed` for the virtual base package. An earlier
seed without it failed dependency selection/file ownership checks and is excluded.
The virtual base provides debconf, but does not have a separate debconf status
record. Consequently apt-extracttemplates prints debconf-version warnings in
both control and candidate trials; the hook remains enabled. These results use
this prepared base rather than a conventional Debian bootstrap.

## Recorded trials

Reports are under `artifacts/node-install-20260930/`.

| Trial | Install | Unpack | Configure | Result |
| --- | ---: | ---: | ---: | --- |
| Original, diagnostic sampling | 512.31 s | 497 s | 8 s | Passed; diagnostic only |
| r1 | 452.74 s | 438 s | 8 s | Passed |
| r3 | 477.90 s | 462 s | 9 s | Passed |
| r5 | 431.37 s | 416 s | 9 s | Passed |
| Original, unprofiled control | 575.38 s | 558 s | 11 s | Passed |
| r7 | 384.30 s | 370 s | 7 s | Passed, including file verification |
| r8 | 359.73 s | 346 s | 8 s | Passed, including file verification |
| r8 repeat, same binaries | 429.33 s | 415 s | 8 s | Passed, including file verification |

Unpack/configure subdivisions come from dpkg's one-second-resolution event log;
the install column measures the complete apt process transaction. The original
report's UTF-8 status file was initially read using the host default encoding;
`profile/validated-report.json` preserves the successful installation timing with
explicit UTF-8 validation. The original report is retained for audit.

Host variability remains material. In particular, r3 regressed
relative to r1 in the complete install even though a subsequent small-file probe
was faster. The original diagnostic run also cannot substitute for a clean
unprofiled control. The later clean control is slower than the original sampled
run, showing material environmental variability. Other user sessions remained
running. Retain both original measurements rather than selecting the slower
control to maximize a claimed improvement.

r7 is 25.0% shorter than the initial 512.31 s observation and 33.2% shorter than
the later clean control. This is one candidate trial, not a distribution of
repeat timings. The remaining six-minute installation is still substantial.
Its separately timed file verification took 20.33 s and produced no differences.
r8 reduced the full install by another 24.57 s (6.4%), to 29.8% below the initial
512.31 s observation. Its separately timed verification took 22.04 s and also
produced no differences. The second r8 installation took 429.33 s, with 36.32 s
separate verification and no differences. Both r8 reports have identical
distribution hashes and identical installed package versions.

The observed r8 range is therefore 359.73–429.33 s: 16.2–29.8% below the initial
512.31 s observation, or 25.4–37.5% below the later 575.38 s clean control.
Two trials are insufficient to establish a reliable latency distribution. The
repeat also overlaps earlier candidates' timings, so do not infer that every
incremental change produces a stable wall-time improvement. Installation still
takes six to seven minutes; the requested very fast installation is not achieved.

The install phases all completed 5,255 native processes. r3 reduced counted page
faults from r1's 50,999,018 to 44,227,152, but this did not reduce installation
latency in that run. Peak job commit stayed around 763 MB under a 6 GiB cap.
r7 recorded 117.44 s user CPU plus 232.02 s kernel CPU, 44,547,394 page faults,
and 763,768,832 bytes peak job commit across the complete checked session.
r8 recorded 109.64 s user CPU plus 216.17 s kernel CPU, 44,238,052 page faults,
and 763,359,232 bytes peak job commit. Total install CPU dropped from r7's
349.45 s to 325.81 s, accompanying the wall-time improvement.
The repeat recorded 116.55 s user CPU plus 243.70 s kernel CPU, 44,260,115 page
faults, and 764,039,168 bytes peak job commit. Thus the two r8 session peaks are
approximately 728–729 MiB; neither run shows a peak-memory increase over the
original approximately 763–765 MB range.

## Implementation

- Reuse the native path already resolved during the current operation; do not
  restart the component walker for metadata operations and opens.
  r8 extends this to unlink, rmdir and rename while retaining
  the mount write leases and the existing descriptor/cgroup dispatch.
- Try one whole-path attribute query for native paths that cross no mounted
  backend. `NtQueryAttributesFile` uses `OBJ_DONT_REPARSE` to reject native
  reparses in ancestors as well as the leaf. Hosted symlinks, mount crossings,
  missing parents, `..` and trailing slashes use the full walker. No pathname
  cache survives an operation.
  A missing leaf beneath a separately verified native directory also reuses
  the resolved path. The final operation retains its normal ENOENT/create and
  synthetic-file behavior, including after chroot.
- Share an existing mount writer gate through a weak reference. The last active
  writer still closes its handle immediately; read-only remounts retain their
  native sharing exclusion.
- Copy ordinary private fork mappings in 64 KiB runs, using SSE2 to omit zero
  runs only when the child storage is newly allocated and zero initialized.
  Retained sections and COW mappings retain their existing copy rules. Native
  protections are restored on success and failure.
- Reuse privately owned metadata opens for chmod/chown and creation-parent
  metadata. Guest descriptor operations still acquire an independent I/O open.
- Create exclusive native files with their initial Linux mode/owner EA passed
  to `NtCreateFile`, publishing file and metadata together. Existing-file opens,
  overlay staging and anonymous files keep their existing paths. Read-only
  projection, setgid inheritance and privilege-bit rules remain enforced.
- Reuse the bounded EA query helper for extended attributes, avoiding a 64 KiB
  allocation/clear for every absent or small attribute table. Large values grow
  only after an explicit native buffer-size result. Chown removes capabilities
  using its existing private metadata open and inode mutex.

The native path flag is documented in Microsoft's
[OBJECT_ATTRIBUTES reference](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes);
the attribute query is documented in
[NtQueryAttributesFile](https://learn.microsoft.com/en-us/windows/win32/devnotes/ntqueryattributesfile).
The NT pathname allocation is released with RAII on every return path.

## Validation

Full installs require configured dpkg status, a working Node JavaScript/crypto
probe, npm version output, and an empty `dpkg --audit` result. Starting with r7,
the harness also requires an empty `dpkg --verify` result in a separately timed
phase. The clean control passed this same check in a separate follow-up session
(`control/verify/report.json`). Each session owns
its processes in a Windows Job with a deadline and commit cap.

Native tests cover mount policy lifetimes, lookup after replacement, hosted and
native links, readonly metadata, existing inode preservation, and sparse/dense
fork copy including unaligned/tail data and callback failure. The guest
`SparseForkStackProbe` checks sparse and dense 2 MiB stacks across fork, private
parent/child writes, protected-page contents and preservation of PROT_NONE.
`AllocationGrowthProbe` also passes with the SIMD fork path.

The r7 and r8 suites each passed 181 native tests across 19 filters
(`native-tests-r7.json`, `native-tests-r8.json`). The r8 extension verifies
single-walk unlink/rmdir, two-walk rename, retained open inode data after
rename/unlink, symlink deletion, missing names, file/directory type errors, and
read-only bind refusal. Guest `NativeLookupProbe`, stack and allocation probes
all passed (`guest-r7/`, `guest-r8/`); the lookup probe checks missing-path errno
behavior, hosted directory links and synthetic `/etc/hosts` after chroot. Its
r8 extension also tests rename/unlink through a hosted directory link while
retaining an open data descriptor. With both r5 and r8,
128 fork plus 128 exec churn iterations completed with the application still
alive at observation: init retained only its standby worker and the application,
with respectively one and zero pending transactions. This checks bounded state
for the tested lifecycle; it is not a claim that all possible leaks are excluded.

The r7 diagnostic `core-js-profile-r7` reinstall is separate from the full
timing trial. The sampler captured actual bounded OBJECT_ATTRIBUTES names at
NtCreateFile in 12 snapshots, including package data, private inode reopens,
and `/etc/passwd`/`/etc/group` reads. Executable-looking stack words are only
candidates, not unwound caller frames. Every sampled thread resumes before
symbol resolution/output; the owning Job bounds process lifetime.

The r8 lifecycle report is `churn-r8/results.json`. The first repeated-run root
setup failed with host access denied and is excluded. A new independently
created root (`r8-repeat2-root`) completed setup successfully; no timed trial
uses the incomplete root. Both complete r8 installation reports passed.

## Reproduction with the retained local fixtures

Build a new distribution with `tools/build.ps1 -Release -NativeOnly -SkipFormat
-SkipTests -TargetDirectory target/apt-syscall -DistDirectory <new-dist>`. Keep
the original control distribution unchanged. For each timed run, use a new
root/output name and wait for compilation and other owned workloads to finish:

```powershell
artifacts/apt-syscall-20260930/candidate/worker.exe setup --root artifacts/node-install-20260930/replay-root --dist artifacts/node-install-20260930/seed
python tools/benchmark-node-install.py --root artifacts/node-install-20260930/replay-root --dist <new-dist> --archives artifacts/goal-systemd-idle/debian-root/var/cache/apt/archives --output artifacts/node-install-20260930/replay --timeout 1200
```

The tool refuses roots already containing Node/npm and existing output folders.
`--sample` is explicitly diagnostic and must not be used for a clean timing run.
