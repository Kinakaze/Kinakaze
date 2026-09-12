# Linux guest dependency cache

The locked payloads now also include nginx, X11/XInput/RandR and ALSA/Pulse
headers, Make/Ninja/pkgconf, Git, sed/grep/find/patch, jq, OpenSSL, rsync, file,
and procps. Native Xi, RandR and Xext are their own modules under rootfs/lib; Debian
headers stay under usr/include. `--package file` includes its magic database,
and RandR headers include the Xrender/X11 header dependency chain. Package
installation and symbol availability are distinct from application acceptance;
consult docs/progress.md for actual passing and failing behaviors.

`--package libc-dev-bin` provides the real GNU `gencat` used by the message
catalog tests. `--package gpg` includes `gpg-agent`; `--package gpgv` adds the
standalone verifier. These locked payloads have passed signature verification,
tamper rejection and an AES256 symmetric round trip in the independent review
root. See `artifacts/review-runtime-acceptance.json` for the precise scope.

`--package openssl` includes the package's default configuration and its
`/usr/lib/ssl/openssl.cnf` alias, both authenticated against the archive. No
OPENSSL_CONF override is needed. A complete SSL stack still needs compatible
library inputs: the current source-root 3.6.4 libcrypto fails the Debian 3.0
certificate-generation probe, while source curl/QUIC require 3.5 symbols.
Do not treat configuration installation as TLS acceptance or silently replace
that entire dependency family with 3.0. See the recorded consumer audit in
`artifacts/openssl-dependency-audit.json`.

The reviewed inputs now also include Debian GCC 12.2, binutils 2.40, compiler
headers/startup objects, Redis 7.0.15 and PostgreSQL 15.19 with their ELF dependencies.
`--package gcc` includes its compiler, cpp, binutils and development payloads;
`--package redis-server` includes the redis-tools executable owner. Cross-package
command aliases contain the authenticated owner's bytes. PostgreSQL uses its
actual `/usr/lib/postgresql/15/bin` location. No maintainer scripts are executed.

Clang 14 and its frontend libraries, resource headers, linker tools, C++ and
Objective-C development inputs are now locked as well (`--package clang`).
Clang's standard library aliases retain the authenticated Debian target bytes;
private LLVM paths remain available. For compiler roots, run the native build
with `-Development` after dependency preparation to publish the project's ELF
link inputs at standard development-library paths. GCC has passed compilation,
linking, mixed PE/ELF loading, pthread/TLS and fork execution. Clang now passes the same actual compiler/runtime probe. Its Debian resource
header directory aliases are materialized from the authenticated package trees,
including the standard include and lib locations. Dependency closure alone
remains insufficient to establish arbitrary program behavior.

Redis has passed real Unix socket transactions, Lua, binary transfer, forked RDB
and AOF persistence, shutdown and restart recovery with the native modules.

The `libc-bin` payload supplies the GNU locale command and authenticated `C.utf8` data
under `/usr/lib/locale`; it does not install Debian's loader or `ldconfig` over
the native module owners. Include `--package libc-bin` for C.UTF-8 locale users.
Native libc validates and loads LC_CTYPE only on first request. The built-in C
locale needs no locale files; unsupported locale names return an error.
`--package tzdata` installs the verified zoneinfo tree and expands its actual
POSIX directory aliases. It leaves `/etc/localtime` to system configuration.
The locale executable now passes listing C.utf8 and reporting its UTF-8 charmap.
This does not imply support for arbitrary locale archives or translated messages.

```powershell
python tools/prepare-root.py --source artifacts/tool-root --root artifacts/expanded-tools-root --dist artifacts/native-direct-dist --package gcc --package redis-server --offline
python tools/prepare-root.py --source artifacts/tool-root --root artifacts/postgres-tools-root --dist artifacts/native-direct-dist --package postgresql-15 --package postgresql-client-15 --offline
```

Relocatable compiler objects are payloads rather than runtime shared libraries.
Package closure verifies inputs; it does not imply compilation/linking or database
service behavior passes. Current application results are in `docs/progress.md`.

`prepare-root.py` closes the Linux x86-64 ELF `DT_NEEDED` graph before copying
the destination root. It examines Java ELF files and Linux ELF members inside
Minecraft native JARs. Native libraries are discovered from the distribution's
`rootfs/lib/` filenames and PE exports; actual ELF files in that directory remain
ordinary guest dependencies. Remaining
dependencies must be real ELF files in the source root or this reviewed lock.
Unknown dependencies stop preflight with their requesting file and SONAME.

Run from `kinakazev2` with Python 3.11 or newer:

```powershell
python tools/guest-deps/fetch.py
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --minecraft --check-only --offline
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/guest-root --minecraft --offline
python -m unittest discover -s tools/guest-deps -v
```

`--java` prepares BusyBox, curl and the JRE. `--minecraft` also prepares the
selected client version, libraries and assets. `--java-home` and
`--minecraft-version` select other already downloaded source artifacts.
`--deps-cache` changes the default `artifacts/guest-deps` cache directory.
Without `--offline`, a missing locked package is downloaded from its official
Debian HTTPS URL. `--check-only` may fill this verified cache but leaves the
destination root untouched. The source runtime is never modified.

The original missing Java native dependency is `libXtst.so.6`, supplied by Debian
bookworm `libxtst6` version `2:1.2.3-1.1`, architecture `amd64`, source package
`libxtst`. Its archive is 27,984 bytes with SHA-256
`be67f1efe7cd3dbe9dc0d717e0a8c0959cc663950aa15a42636ab90b134f9cb5`.
The official [download metadata](https://packages.debian.org/bookworm/amd64/libxtst6/download)
and [package](https://deb.debian.org/debian/pool/main/libx/libxtst/libxtst6_1.2.3-1.1_amd64.deb)
are recorded in `dependencies.lock.json`, along with the source package path,
individual ELF SHA-256, actual symlink target and copyright document hash.

The fetcher uses Python's standard library. It checks the archive hash, Debian
control package/version/architecture/source fields (exactly the locked `amd64`
or architecture-independent `all`), each selected file hash,
ELF architecture and SONAME. It parses archives in memory and materializes only
the selected files. Package scripts do not run; symlink aliases become regular
files containing the verified ELF bytes. No Linux package manager or elevated
Windows privileges are needed. Every installed file and external package is
recorded only when an external `--report PATH` is requested. No runtime inventory
is placed in the guest root. A locked library may have a lookup `name` differing
from its ELF SONAME, as Debian compatibility aliases do; both the real SONAME and
authenticated file/link target are still checked.

To add a dependency, inspect the requesting ELF and its architecture, select a
version from an official Debian repository, verify the archive against that
repository's package metadata, and add its precise URL, source package/version,
archive hash, selected file hashes and copyright file to the lock. Run the
offline tests and full profile preflight after fetching. The preparer does not
automatically choose a package or substitute a Windows DLL for an unprovided
Linux SONAME.

Java fontconfig and Minecraft udev are explicit `dlopen` seeds. Other optional
backends loaded only by application configuration require their own reviewed
dependency entries when exercised. Native dependency closure does not prove
that fonts, graphics, audio, authentication or the complete game run correctly.

`--package NAME` also seeds reviewed executables and resource trees. The lock now
contains SQLite 3.40.1-2+deb12u2, Python 3.11.2-6+deb12u8 (interpreter and standard
library), libcrypt1 1:4.4.33-2, iptables 1.8.9-2 (`amd64`), and netbase 6.4
(`all`) from the official bookworm package index. `netbase` supplies guest network
databases, including `/etc/protocols` and `/etc/services`; protocol lookup does
not synthesize TCP for unknown names or absent files.
Named `requires` entries include the other reviewed payload packages; ELF
dependency resolution uses explicitly seeded libraries before searching the
source root, avoiding accidental selection of an older copy of that SONAME.

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/common-root --package netbase --package sqlite3 --package python3.11-minimal --package libpython3.11-stdlib --program usr/sbin/sshd
python tests/guest/tool-matrix.py --root artifacts/common-root --only sqlite --only sqlite-transactions --only python --only python-runtime --only sshd --report artifacts/tool-matrix-common.json
python tests/guest/run-time-globals.py
```

Each `trees` selector is limited to a validated package-member prefix. Contents
come from the hash-verified archive, cached files are individually checked, and
installed files receive hashes in the optional external report. An absolute Debian
symlink is resolved against the guest archive member map, never the host. Python's
cross-package sysconfig alias is explicitly materialized from its owning minimal
package; its dangling alias in the stdlib archive is an exact locked exclusion.
Arbitrary links outside the package are not followed. Maintainer scripts still do
not run. These are curated payload dependencies, not an APT dependency solver.

When a CA bundle is available, preparation also supplies OpenSSL's compiled
default `/usr/lib/ssl/cert.pem`, with the same input hash as the bundle. The
runtime no longer injects a Python installation directory; Python locates its
installed standard library normally. Explicit guest environment overrides remain
available for alternate installations.

SQLite has passed in-memory queries and on-disk WAL/commit/rollback/reopen,
constraint, and integrity checks. Python has passed positional vector I/O,
thread pools, asyncio TCP/subprocesses, Unix socket readiness, archive/compression,
SQLite transactions and Unicode child-process environment/cwd. The optional
`curl-smoke.py --python` also verifies actual Python HTTPS binary transfer and
rejection of untrusted certificates and mismatched hostnames. The default CA
store is checked to contain certificates. `tests/guest/run-sshd.py` now verifies a
real host OpenSSH client login with isolated keys, remote commands, pipes and exit
status 23. This fixture uses key authentication with PAM disabled; PTY, SFTP and
seccomp isolation still need separate verification.

Docker inputs include the reviewed iptables nft executable aliases and complete
xtables plugin tree. Package-member links are resolved to verified ELF bytes;
legacy source-root NTFS alternate-stream links are not a supported interchange
format. Do not overlay old iptables executables/plugins on package-owned paths.

```powershell
python tools/prepare-root.py --source E:/Naka/crysoacu/target/debug --root artifacts/docker-root --package netbase --package iptables --program usr/bin/docker --program usr/bin/dockerd --program usr/bin/docker-proxy --program usr/bin/containerd --program usr/bin/containerd-shim-runc-v2 --program usr/bin/ctr --program usr/bin/runc --offline
python tests/guest/tool-matrix.py --root artifacts/docker-root --manifest tests/guest/docker-matrix.json --only docker-daemon-api --report artifacts/docker-api.json --timeout 150
python tests/guest/tool-matrix.py --root artifacts/docker-root --manifest tests/guest/docker-matrix.json --only docker-container-lifecycle --report artifacts/docker-container.json --timeout 240
```

The daemon/API probe has passed with default networking/storage settings and
complete temporary-directory cleanup. The container probe has imported its local
BusyBox image, started a real container, verified PID 1/proc, observed a bind-volume
write and obtained a default bridge IP. Correct mount-setns root/cwd installation
and preservation of subreaper adoption across exec now let the complete lifecycle
probe pass: exec, exact exit status 23, a second start/exit, container/image removal,
and temporary-directory cleanup after daemon shutdown. The daemon still logs
shim/runc cleanup, cgroup and network warnings. Actual container network traffic
and resource reclamation while the daemon stays alive need separate verification;
this probe does not establish complete Docker compatibility.
