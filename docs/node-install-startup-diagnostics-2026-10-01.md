# Complete Node installation startup diagnosis

The immutable R6 distribution completed a fresh installation of all 357 packages
with startup spans, fork phase timers, exec tracing and a retained native-process
census enabled. Node.js/npm, dpkg audit and complete file verification all passed.
The diagnostic install took 290.299 seconds; verification took 17.224 seconds.
This is a diagnostic result, not a new uninstrumented acceptance measurement.
The independently recorded R6 run after a quiet start remains 294.490 seconds,
above the 180-second target.

The distribution and seed match the R6 correctness measurements. Native fork
transfer and preparation were enabled. No compilation was started by this chat
during the run. This diagnostic did not collect a separate host-contention
timeline, so it makes no claim about a quiet host. Normal payloads, package
scripts, triggers and durability operations remained enabled.

## Fork costs after native preparation

The installation interval contains 3,399 timed forks. Membership is determined
from each fork's UTC handoff-wait timestamp; a boundary operation can straddle
phases. The full session census retained 5,432 native process lifetimes and
matched 3,412 fork adoptions using process birth times, including workers whose
native parent is init rather than their logical guest parent.

| Installation fork phase | Sum of spans | Median per fork |
| --- | ---: | ---: |
| Acquire/create native candidate | 0.576 s | 89 us |
| Candidate readiness wait | 0.507 s | 12 us |
| Allocator arena copy | 3.040 s | 835 us |
| Other mapping copy | 16.115 s | 2,178 us |
| Fork copy operation, including the above | 23.208 s | 4,744 us |
| Handoff restoration wait | 30.519 s | 8,593 us |

The preparation pool has largely removed native creation from the parent's
foreground fork path. Increasing its capacity cannot remove the separately
measured copying and restoration work. The child restoration participants and
provider spans are preserved in the machine-readable measurement.

## Exec costs

Shared stderr records can interleave. The analyzer accepts only complete,
strictly ordered phase sequences with monotonic elapsed time and rejects
partial or corrupted sequences. It retained 1,517 of 1,879 exec start records
for the whole session, including non-install phases. The last visible phase is
identity release; descriptor closure can close the diagnostic stderr itself.

| Exec phase increment | Median of complete sequences |
| --- | ---: |
| Executable resolution/read | 0.551 ms |
| Handoff serialization | 0.377 ms |
| Create suspended native process | 5.782 ms |
| Publish process identity and resume | 1.937 ms |
| Wait for replacement to own handoff | 17.983 ms |
| Wait for validated image and commit identity | 15.870 ms |
| Entry through identity release | 44.330 ms |

These medians describe the retained complete sequences, not a sum that can be
subtracted from installation time. In particular, image validation, linking,
handoff restoration and parent waits overlap. Exec still creates a fresh native
replacement. A future preloaded replacement needs explicit descriptor transfer,
one-time consumption, recoverable pre-commit failure and the existing old-image
death barrier before guest execution.

During installation, 5,391 native runtime-load spans totalled 33.768 seconds and
5,371 provider-load spans totalled 18.623 seconds. Some belong to background
fork preparation; their totals are not foreground wall-time savings.

## Process accounting and bounded stack sampling

Whole-process CPU was classified with birth-matched exec ancestry and actual
fork adoption edges. Installation dpkg processes used 94.078 CPU seconds;
fork children of dpkg-deb used 61.531 seconds and fork children of dpkg used
61.016 seconds. These include useful guest work and startup. The init lifetime
used 39.047 CPU seconds, including native fork preparation and session work.

A separate bounded late-unpack sample captured 64 main-dpkg native stacks:
30 were in waitpid, six in blocking reads, five in fork completion waits,
seven in native memory copying, four in file creation and four in file flushes.
This selection describes that interval, not the whole transaction or CPU-only
samples. Unwinding stops at the first non-PE frame; nearest exported symbols
with large offsets are not treated as exact private function identities.

The census and local repository used 3.797 observer CPU seconds. The separate
stack observer used 2.813 CPU seconds. Trace writes and span bookkeeping run
inside the measured processes and are not subtracted from their counters.

Evidence under `artifacts/goal-install-180-20260930/`:

- `run-r6-startup-census.py`, `sample-r6-install.py`, `analyze-r6-startup.py`
- `r6-startup-census/fixture/report.json`, `r6-startup-census/processes.json`
- `r6-startup-census/analysis.json`, `r6-startup-census/fork/`
- `r6-startup-census/fixture/session/profile/`
- `r6-startup-census/native-samples/`
- `r6-dist.json`, `r6-source.json`, `r6-startup-setup.log`

The committed measurement includes SHA-256 references to these reports. Nested
and concurrent spans must not be added as predicted wall-time reductions.
