# Remote COW copy route validation

Retain the single coalesced production implementation. Four invocations of
one unchanged native test binary compared the exact old function with the
production candidate. Each invocation ran three ABBA rounds with eight copies
per sample. All 480 samples passed complete destination-byte, copied-byte and
copy-call checks. The underlying shared section remained unchanged.

The destination is a real PAGE_WRITECOPY view in another suspended Windows
process. Its own main thread never runs. Process creation, mapping/unmapping
and complete ReadProcessMemory validation are outside the copy timers. Warm
views retain their private pages; cold views are freshly mapped before every
copy. The helper is hidden, terminated and waited after the test.

| Destination | Size | Old μs | Coalesced μs | Median reduction |
| --- | --- | ---: | ---: | ---: |
| Warm | 64 KiB | 5.612 | 5.588 | 0.45% |
| Warm | 1 MiB | 75.312 | 75.731 | -0.56% |
| Warm | 2.031 MiB | 160.800 | 153.631 | 4.46% |
| Warm | 4 MiB | 355.081 | 325.394 | 8.36% |
| Warm | 16 MiB | 2621.463 | 2216.481 | 15.45% |
| Cold | 64 KiB | 20.944 | 20.631 | 1.49% |
| Cold | 1 MiB | 305.975 | 305.281 | 0.23% |
| Cold | 2.031 MiB | 618.281 | 612.131 | 0.99% |
| Cold | 4 MiB | 1262.419 | 1250.838 | 0.92% |
| Cold | 16 MiB | 5538.394 | 5591.881 | -0.97% |

For warm destinations, all four invocations favored coalescing in each of
the three larger ranges. Aggregate median reductions were 4.46%, 8.36% and
15.45%. Small warm ranges use one copy call in both implementations and are
effectively unchanged.

Cold results change sign between invocations and are effectively unchanged.
The initial 2.031 MiB cold result regressed by 5.84%; the subsequent invocation
differences were −0.67%, −0.98% and +5.04%, with an aggregate difference of
+0.99%. No cold-copy gain is established. The first observation is retained,
and the repeats resolve this specific uncertainty rather than replacing it.

This comparison strengthens the [local-view evidence](fork-cow-coalescing-2026-10-01.md)
for selecting coalescing, but does not prove a complete fork or APT wall-time
improvement. The latest [integrated installation](integrated-selected-cow-install-2026-10-01.md)
completed its historical checks in 292.443 seconds under observed competing
work, but failed debconf preconfiguration. That run and the historical
185.597-second result are not accepted complete-work comparisons. The current
working-preconfiguration reference is R10 at 215.415 seconds; the 180-second
goal remains unmet. These remote copy tests do not use the guest seed.

[Raw samples and provenance](measurements/fork-cow-remote-2026-10-01.json)
preserve every observation, per-invocation comparisons, binary hashes and
diagnostic source hashes. The baseline is confined to the ignored artifact
test source; production has one copy implementation and no route selector.
