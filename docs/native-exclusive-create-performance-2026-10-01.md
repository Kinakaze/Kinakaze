# Native exclusive creation, 2026-10-01

Ordinary `O_CREAT|O_EXCL` writable opens now pin one fresh native parent directory, read only its Linux inode record and atomically create the child with its initial metadata. This avoids the component walk and full parent stat transaction on eligible native paths. The retained parent and data handles preserve inode identity through rename, replacement and unlink. Live filesystem credentials, umask, setgid inheritance, mount writer accounting and fs-verity writable exclusion remain in use.

The path is enabled by default; `KINAKAZE_NATIVE_EXCLUSIVE_CREATE=0` selects the existing walker. It requires DAC override and an ordinary root native mount. Dot components, explicit relative dirfds, trailing slashes, special mounts, overlay roots and reparse paths use the walker. No metadata observation persists beyond one operation. Flush behavior is unchanged.

## Same-binary comparison

The frozen release distribution was alternated with switch order `0,1,1,0,1,0`. Each scenario performs 2000 exclusive creates, writes 16 bytes and closes the descriptor. Parent Linux metadata is initialized before timing. After timing, every file's contents and mode are checked and it is removed. The body excludes interpreter startup and cleanup; this microbenchmark does not call fsync. The guest lifecycle regression separately exercises write and fsync, including a newly created mode-0400 file.

| Path | Body median, off | Body median, on | Reduction | Whole-job CPU, off / on |
| --- | ---: | ---: | ---: | ---: |
| Absolute | 743.607 ms | 707.228 ms | 4.89% | 2031.250 / 1921.875 ms |
| Cwd-relative | 918.868 ms | 859.820 ms | 6.43% | 2609.375 / 2593.750 ms |

There are three observations per scenario and switch state. All 12 rows passed, with unchanged distribution hashes. Foreign compilation was present during the measurement; raw host samples are retained. These are modest local gains. They do not establish a whole-install improvement or attainment of 180 seconds.

## Correctness and provenance

62 native regressions passed; one pre-existing native capability diagnostic remains ignored. Seven new native tests cover live parent metadata and setgid inheritance, retained parent identity, existing leaf rejection, guest and native links, descriptor flags, verity exclusion and final/ancestor junction rejection. An initial prototype accepted a final junction; the dedicated regression caught it. The validated implementation omits `FILE_OPEN_REPARSE_POINT` from the parent open so `OBJ_DONT_REPARSE` rejects it. The failed prototype evidence is separate from the accepted evidence.

30 guest checks passed, 15 in each switch state, covering creation and writable lifetime, permissions, path metadata, native reads/lookups, fork/vfork/exec and robust pthread aliases. The production build checked 29 native modules and 5971 guest exports. All 24 export-generator tests passed.

The build source is detached `ca8020e20599e112c7694c6301cbd95be9677384` plus the six implementation files, baseline formatting and the already committed export-generator repair from `22df00b` (copied from `8adef3b`). This source predates the later shared PI implementation and is not a measurement of current integrated main. The exact 36-file delta is archived at `artifacts/apt-180-20260930/r14-native-create-packaged-sources.zip`; its manifest and hash are embedded in [the measurement](measurements/native-exclusive-create-2026-10-01.json). Only the six implementation files are backported; unrelated formatting and already committed export declarations are excluded.

The measurement records all source/distribution hashes, raw benchmark and regression results, and evidence hashes. Reproduce with `python artifacts/apt-180-20260930/run-r14-native-create.py validate` and `compare`, using a new output directory. The current latest accepted integrated full install is separately documented at [215.415 seconds](goal180-r10-integrated-install-2026-10-01.md); its different source and host conditions prevent causal subtraction from this microbenchmark. A fresh complete install of this frozen candidate is pending.

The native parent operation follows the documented [OBJECT_ATTRIBUTES](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes) root handle and reparse behavior and [NtCreateFile](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile) create options. Native junction tests also verify the actual host behavior.
