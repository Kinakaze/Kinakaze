# Deferred provider metadata after fork

Fork restoration now retains each already loaded provider at its recorded
address and delays rebuilding its symbol declarations until a symbol lookup
or introspection call needs them. The registry retains the immutable source
files immediately; an owning Windows module reference retains the mapping.
A moved module is rejected before guest execution. Missing native mappings and
generic adapters keep eager reconstruction. Metadata failures are cached and
reported through the existing loader error paths.

`KINAKAZE_FORK_PROVIDER_METADATA=eager` retains immediate reconstruction for
same-binary comparisons. The default uses deferred reconstruction. Reference
counts, nodelete, symbol versions, COPY redirection and nested fork preserve
their existing behavior.

Native declarations also reuse verified name order for lookup and validation.
Unordered adapters still build an index and receive the complete duplicate and
alias checks. Materialization groups borrowed declarations by guest name and
version, including names such as `atan2` that interrupt PE lexical adjacency.
Provider registration obtains canonical paths from the existing source-file
handle instead of opening each path again. These changes do not enable or
depend on the separate shared native catalog.

Validation on 2026-10-01 passed 47 focused native tests and the full release
workspace build. Packaged guest checks passed for provider metadata, nested
fork/exec, native capability transfer, concurrent vfork, exec preparation,
sparse stacks, shared futexes, signal restart, Unix rights and allocation.
Late initial-exec TLS loading passed separately. Idle preparation workers used
zero measured CPU milliseconds during the one-second observation interval.

The initial shared-futex probe failed because its old timed-wait expectation
had not followed main's signal-semantics correction. The expected result was
aligned with the already corrected implementation, and that probe plus the
30-case signal-restart probe passed against the unchanged frozen distribution.
The original failure remains in the evidence. No runtime change was made to
make that test pass.

Measurements and artifact references are in
[the provider measurement](measurements/fork-provider-metadata-2026-10-01.json).
Four unprofiled eager/lazy/lazy/eager runs each completed 128 iterations per
path. They did not establish a wall-time improvement: eager/lazy median pure
fork times were 3.138/3.323 seconds and fork+exec times were 6.109/7.001 seconds.
The unchanged posix_spawn control also varied substantially, including one
10.027-second sample versus 4.489 seconds in the following run. All samples
are retained; none is excluded to manufacture a speedup.

A separate diagnostic pair recorded 32 forks per mode. Median facade metadata
restoration was 1,621 us eager and 21 us deferred, while registry restoration
remained 7,269.5/7,051.5 us. Total loader-participant medians were
10,297/7,912 us. These instrumented spans establish that reconstruction is
deferred; they are not acceptance timings or predicted installation savings.

The separate complete-install result for exec preparation is 266.655 seconds;
it predates this provider change and does not establish its install benefit.
The 180-second installation target remains unmet.
