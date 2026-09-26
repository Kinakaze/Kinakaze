# Debian ABI validation — 2026-09-26

This revision closes the native ABI symbol gaps observed in the locked Debian
standard image. It does **not** establish complete Linux kernel or Debian service
compatibility. Version names remain exact; no wildcard symbol versions are used.

## Image and evidence

- The offline manifest installs 302 locked packages, including APT/dpkg, their
  dependencies and configuration, 16,054 files and 3,120 VFS links.
- Native providers export 5,901 guest symbols. The audit of 1,164 ELF objects and
  10,565 versioned requirements reports **zero missing native symbols/versions**.
  External ELF dependencies such as libtirpc remain supplied by the locked image.
- [Static ABI audit](debian-abi-symbol-audit.json) records the observed dependency
  set. [Runtime evidence](debian-abi-validation.json) pins native image, manifest,
  probe source and ELF hashes and records the probe and lifecycle results.
- [Command results](debian-abi-command-results.csv) retain each invocation's
  outcome, purpose and failure evidence. Aliases count as inventory entries;
  displaying help is not treated as a functional pass.

## Command coverage

| Check | Passed | Usage only | Failed | Timeout | Not tested |
| --- | ---: | ---: | ---: | ---: | ---: |
| Startup/help/version | 565 | 147 | 18 | 0 | 28 |
| Functional scenarios | 436 | — | 72 | 6 | 244 |

There are 758 command entries, including 105 aliases and 657 distinct targets.
739 entries received at least one check; 514 received a functional scenario.
Both phases report **zero missing-symbol failures**. Compared with the baseline,
startup passes increase from 515 to 565 and functional passes from 420 to 436.

The final parallel run initially had 435 functional passes and 7 timeouts.
`toe -a` exceeded the 12-second deadline while enumerating terminfo; its recorded
serial retry with a 30-second deadline passed. The original JSONL is retained in
`artifacts/debian-command-coverage-final`. Counts above include that explicit retry.

Remaining failures include service/kernel interfaces, package configuration and
runtime behavior; they have not all been assigned root causes. The failure labels
in [the CSV](debian-abi-command-results.csv) are diagnostic categories, not proofs
of the cause. The baseline audit and its results remain preserved separately.

## Implemented behavior

- Preserve x87 long-double range and precision in narrow/wide formatting and
  `nearbyintl`, including directed rounding and the floating-point environment.
- Read the guest's shadow, group-shadow, tty and login databases; honor reentrant
  buffer sizes and cursor retry behavior. Parse NSS records and nsswitch actions,
  and keep independent netgroup enumeration state.
- Implement DNS name compression, scratch buffer growth, temporary name selection
  and shell word expansion with quoting, append/reuse and NOCMD validation.
- Deliver queued signals across guest processes, preserve real-time FIFO order
  and 64-bit payloads, coalesce standard signals and discard ignored queues.
  Blocked queued signals no longer interrupt an unrelated blocking pipe read.
- Publish modern POSIX message-queue versions from libc. Add legacy nocancel,
  memory, secure-environment and private lock interfaces.
- Forward SunRPC/XDR through the bundled libtirpc implementation, with real
  encode/decode and UDP RPC behavior. Handle an unbound Winsock socket's local
  address without allocating a port as a side effect of getsockname.
- Resolve native ELF TLS symbols through a real TLS module. Direct ELF `errno`
  imports and `__errno_location()` address the same cell; threads and forked
  processes retain independent values.
- Provide the pinned glibc loader data needed by libmvec IFUNCs, typed tunable
  lookup, C-locale transliteration, trusted-host file checks and a mapped
  program-break arena with fork preservation.

## Behavioral regression

`tests/guest/run-debian-abi.py` installs a separate complete root whose path has
spaces and clears the host PATH. The Linux ELF probe exercises all groups above,
including a loopback UDP server that checks RPC headers and arguments, a contended
private lock, native TLS pthread/fork isolation, protected gconv callback pointers,
short-buffer retries, actual vector sine computation, and brk shrink/regrow zeroing.
It passes with `DEBIAN_ABI_OK`; the RPC request is -17 and the validated reply -16.

Release runtime acceptance passes all eight checks: default offline installation,
concurrent first startup, external-manifest installation of native and guest files,
configuration preservation and exit propagation, insertion under a live init-tree
parent with PPID/cwd/environment, missing-parent rejection, detached child lifetime,
and stale-session rejection. These tests install three independent fresh roots.

One build-time root publication returned Windows error 5 without identifying the
failing operation. Repeating the same build succeeded; the independent fresh-root
regression and all three acceptance installations also succeeded. This transient
failure has not been assigned a confirmed cause.

## Compatibility limits

- The private glibc layouts and tunable IDs target the locked Debian glibc 2.36
  build. This is not a promise of arbitrary `GLIBC_PRIVATE` compatibility. Tunable
  lookup does not imply every native subsystem implements that tunable's policy.
- C-locale transliteration uses the default `?` replacement. The gconv inventory
  lists supported built-in codecs; additional Debian gconv tables/modules are
  not all implemented.
- The program break uses its own lazily committed 4 GiB virtual reservation;
  native malloc has a separate allocator. `__libc_freeres` frees the new NSS and
  tunable caches, not every allocation made by every provider.
- SunRPC compatibility uses libtirpc's service state. Linux profiling via
  `LD_PROFILE` is not activated; its mcount compatibility callback is inert.
- Symbol availability alone does not implement systemd/D-Bus services, all
  netlink/sysfs/procfs interfaces, terminals, device control or package postinst
  behavior. The remaining command failures are recorded, not hidden by the ABI
  pass or converted into stub successes.

## Build and review

The workspace release build, export generator check, 22 export tests, 10 bridge
tests, Rust formatting and whitespace checks pass. Workspace Clippy completes
without errors, but reports existing and new style/documentation warnings;
this revision does not claim a warning-free codebase.

The main working tree also contains independent startup-loader work. The TLS
provider changes have been integrated with its deferred symbol resolution, while
preserving those changes. The distributable is built from this task's clean,
isolated commit; unrelated uncommitted changes are not bundled.

## Reproduce

From the repository, with the locked build and package caches available:

```powershell
cargo build --workspace --lib --release --locked --features kinakaze-v2-runtime/guest-engine --target-dir <target>
python tools/native-exports/generate.py --image-dir <target>/release
cargo build --workspace --release --locked --features kinakaze-v2-runtime/guest-engine --target-dir <target>
```

Use the statically linked entry executables and distribution packaging procedure
in [first-run.md](first-run.md), then run:

```powershell
python tools/native-exports/audit.py --observe <dist>/rootfs --dist <dist> --output <results>/symbols.json
python tests/guest/run-debian-abi.py --dist <dist> --link-dir <target>/release/elf-imports --output <results>/abi
python tools/test-release-runtime.py --dist <dist> --report <results>/runtime.json
python tools/test-debian-commands.py --dist <dist> --output <results>/commands --phase smoke --jobs 6
python tools/test-debian-commands.py --dist <dist> --output <results>/commands --phase functional --jobs 6
python tools/report-debian-commands.py --results <results>/commands --csv <results>/commands.csv
```

The command phases use separate guest roots and bind their reports to the worker
and manifest hashes. If a case needs a serial retry, preserve the original JSONL
and use the harness's `--only` and `--merge` options to record it explicitly.
