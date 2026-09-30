# Native path metadata operations

Eligible native namespace paths now keep one opened inode through chmod,
chown or timestamp mutation. The operation retains its mount writer until the
handle closes. Hosted links, mount attachments, overlay paths, dot components
and trailing slashes use the existing resolver. Neither pathname results nor
mutable inode metadata are cached between operations.

The native open rejects reparses in every component with OBJ_DONT_REPARSE, so
an ancestor junction cannot bypass the guest resolver. Rejection and opening
belong to the same native lookup; there is no separate pathname precheck.

The libc chmod and fchmodat entries now share the VFS path. A trailing slash on
a regular file reports ENOTDIR before changing its mode. Builtin device nodes
retain their existing handling.

`KINAKAZE_NATIVE_METADATA=0` disables the new native open path for comparison.
In an alternating disabled/enabled/enabled/disabled Release measurement, each
operation performed 3,000 mutations and checked the resulting file state:

| Operation | Disabled 1 | Enabled 1 | Enabled 2 | Disabled 2 |
| --- | ---: | ---: | ---: | ---: |
| chmod | 625.1 ms | 477.9 ms | 515.7 ms | 619.3 ms |
| chown | 588.1 ms | 483.6 ms | 485.3 ms | 611.5 ms |
| utime | 622.9 ms | 515.6 ms | 579.5 ms | 614.1 ms |

The associated 512-file dpkg checks took 2.238, 2.159, 2.738 and 2.249 seconds.
Other host workloads were active, and the tested integrated build included
additional changes. These measurements support the local operation improvement
but do not establish a full-install speedup for this patch.
They also precede the final ancestor-junction correction; the final native
open is validated separately and those timings are not its acceptance result.

Native regression tests cover replacement and unlink while retaining the
original inode, hosted-link fallback, namespace prefixes, escaped names and
native ancestor junctions.
The chmod test checks trailing-slash errors for both libc entry points.
`tests/guest/NativePathMetadataProbe.py` additionally checks hard links, relative
paths, chroot, read-only bind mounts and the visible mode/owner/time changes.
That probe, NativePermissionProbe and NamespaceServicesProbe passed with the
optimization enabled and disabled in the integrated Release build.
