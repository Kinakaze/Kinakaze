# Retained process identity capabilities

The process table now reuses the workspace's bounded liveness implementation.
Each native thread retains at most 64 non-inheritable process handles. A new
handle is checked against the requested creation time. Every subsequent lookup
still checks that same kernel object's signalled state; it does not cache an
answer that a process remains alive. Exited owners are removed, collisions
replace one slot, and lookups during TLS destruction use an independent open.

Ancestry checks, sweeps and subreaper adoption use this identity check. A sweep
that removes no rows leaves its existing indexes intact. The current process's
immutable creation token is retained separately. Capability requests with PID
zero select the caller directly, avoiding an unnecessary namespace lookup.

The release passed 38 native process-table tests, including recycled identity,
exit code 259, orphan reclamation, zombie retention and TLS destruction.
Twelve packaged guest probes passed across fork/exec, exact wait status,
credentials, namespace operations, native descriptor lifetime, robust mutexes,
signal interruption and sparse/allocation restoration. An additional capability
probe checked PID-zero versus explicit-self queries, updates, error cases and
fork isolation. Idle preparation workers used zero Job CPU milliseconds in the
one-second check. All 59 distribution hashes were unchanged.

The integrated build includes the exclusive-create preflight change and the
main-branch robust-export generator repair. The generator's 23 tests and its
5,969-export validation passed; the robust-mutex guest test passed as well.
No complete-install speedup is claimed from these checks. The last accepted
full installation remains 220.020 seconds and the 180-second goal remains unmet.

Raw build, regression and frozen distribution records are under
`artifacts/apt-180-20260930/root-liveness-*`. The
[source and validation manifest](measurements/retained-process-capabilities-2026-10-01.json)
records hashes for review.

## Complete installation observation

The frozen release from `449d3c3` completed all 357 packages in **261.899 s**.
Download took 2.137 s separately; the complete installed-file verification
took 18.615 s. Node, npm and dpkg audit passed. Installed versions exactly
matched the preceding 220.020-second run, and all 59 distribution hashes
were rechecked unchanged.

This run did not improve the best observed installation. Its Job used
344.578 CPU seconds (117.922 user, 226.656 kernel), created 5,275 processes,
and recorded 41,812,203 page faults. The bounded pre-run check failed to find
20 continuous quiet seconds within two minutes. During installation, 108
of 261 host samples contained a compiler; median host CPU was 53.6%.
The observer consumed 0.844 CPU seconds. These observations cannot isolate
the performance effect of retained capabilities or exclusive creation.
The 180-second target remains unmet; the best complete observation remains
220.020 seconds. Future comparisons also need to track competing runtime
workers, since compiler absence alone does not establish a quiet host.
