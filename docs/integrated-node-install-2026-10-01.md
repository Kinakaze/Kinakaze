# Integrated Node.js/npm installation

Source `5b9c117b923dfa939a289796470de44852f95105` completed the full cached
installation of 357 packages in **220.020 seconds**. Download preparation was
measured separately at 3.702 seconds. All maintainer scripts, triggers and
synchronous writeback remained enabled. Node.js, npm, dpkg audit and the full
package-content verification passed; verification took another 17.549 seconds.
Installed package names and versions exactly match the previous control.
The 180-second installation target remains unmet by 40.020 seconds.

The frozen distribution integrates native fork and exec preparation, deferred
provider metadata, the shared native catalog and the VFS metadata capability
changes. All 59 distribution hashes were rechecked after testing. The coherent
release passed 85 distinct native regression tests and 12 packaged guest checks;
one native capability diagnostic was ignored. Trace and profiling flags were
absent during the installation.

The install Job recorded 105.766 seconds of user CPU and 201.516 seconds of
kernel CPU, 5,275 processes, 41,791,643 page faults and 12,220,655 other I/O
operations. The second-resolution dpkg timestamps attribute 211 seconds to
unpacking and six to configuration.

The previous full exec-pool observation was 266.655 seconds. Its source and host
conditions differ, so the 46.635-second difference cannot be assigned to one
change. This run began after a 20-second quiet-host prerequisite, but unrelated
compilers were subsequently observed in 22 of the 220 installation samples.
Median total host CPU use was 44.05%. This task ran no concurrent build or
benchmark. The observer used 0.609 seconds of CPU over the complete driver run.

[Measurement and evidence hashes](measurements/integrated-node-install-2026-10-01.json)
include the distribution hashes, counters, host observations and validation
references. The earlier catalog comparison remains documented separately in
[the shared catalog measurement](shared-native-catalog.md).
