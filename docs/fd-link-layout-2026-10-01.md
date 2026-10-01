# Compact shared descriptor-link headers

Comparison update: the historical 185.597-second result uses the older seed
with failed debconf template extraction. It is not a like-for-like baseline
for the current working-preconfiguration fixture; see the
[seed comparison](selected-native-open-r13-2026-10-01.md#comparable-installation-fixture).

Production now retains only the compact layout; the legacy route and
environment switch described in this historical comparison were removed.
See the [selection and full-install measurements](fd-link-layout-selection-2026-10-01.md).

The process table now keeps its 8,192 descriptor-link headers contiguous,
with the existing 1,024-byte pathname slots in a separate region. A newly
mapped process can scan 128 KiB of headers without touching the pathname
pages. Table capacity, total mapping size, PID namespace offset, locking,
publication order, lookup, deletion and fork-copy semantics are preserved.

The shared section and mutex names advance from v15 to v16, and the process
table magic advances with them. Build and deploy init, worker and runtime
together. The first table creator selects its layout and writes the stride
into the header; every later process follows that header. The default is
compact. `KINAKAZE_COMPACT_FD_LINKS=0` selects the old interleaved layout for
same-binary comparisons in a new domain.

## Measured cold-view scans

An ignored native diagnostic calls the production `clear_fd_links` on 500
fresh mappings of one committed shared section, retaining six foreign links.
It runs legacy/compact/compact/legacy and checks that those links survive.

| Run | Header stride | Elapsed | Page faults | Kernel CPU |
| --- | ---: | ---: | ---: | ---: |
| Legacy A1 | 1,040 B | 612.603 ms | 1,043,008 | 515.625 ms |
| Compact B1 | 16 B | 19.494 ms | 17,500 | 31.250 ms |
| Compact B2 | 16 B | 19.578 ms | 17,500 | 15.625 ms |
| Legacy A2 | 1,040 B | 582.806 ms | 1,043,000 | 453.125 ms |

These results establish a reduction in the isolated cold-scan cost. They
do not establish an APT installation speedup. No complete install with this
candidate was started before the user requested push and stop. The prior
best verified full installation remains **185.597 seconds**; the sub-180
second goal remains unmet.

## Validation and provenance

- 40 native process-table tests passed with each layout; the diagnostic is
  ignored in normal runs and was executed separately.
- New tests cover maximal and short targets, clearing stale pathname bytes,
  neighboring records, namespace boundaries, collision wraparound,
  replacement, deletion and fork copies under both layouts.
- Init/protocol tests: 52 passed, two pre-existing ignored diagnostics.
- All 23 packaged standalone guest probes passed, including fork/exec pools,
  concurrent vfork, exit/reap, proc descriptor paths, namespace credentials,
  Unix descriptor transfer, filesystem boundaries and native open/create.
- Idle worker inventory and CPU checks passed. All 59 distribution hashes
  were unchanged. Export validation checked 29 modules and 5,973 exports.

An initial generic invocation passed five probes, then invoked
`ProcessChurnProbe` without its required host coordination arguments. It
failed on argument unpacking. The completed standalone suite excludes that
probe; this result does not claim host-coordinated churn coverage. Both
reports are retained.

The coherent release and rebuilt static entry points were compiled from
base `458f9d30d489ec1b5a920691db5149c78ba1e65b` plus the three recorded source
files. Build logs, source snapshots, diagnostic output, guest reports and
distribution hashes are preserved in
`artifacts/apt-180-20260930/root-fd-layout-*`. Raw source snapshot hashes use
the exact build bytes, before Git newline normalization.

[Measurement record](measurements/fd-link-layout-2026-10-01.json) records the
source, artifacts and remaining full-install comparison requirement.
