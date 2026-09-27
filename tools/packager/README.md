# Native packaging

```powershell
./tools/build.ps1 -DistDirectory artifacts/portable-dist
```

The packager reads standard PE exports and COFF import libraries. The import
library supplies each module's native filename. Original DLL bytes are copied
unchanged to `rootfs/lib/<name>.so`; required toolchain DLLs retain their original
names in the same directory. The distribution contains `init.exe`, `worker.exe`,
`rootfs/`, and `rootfs.manifest.json`. The manifest configures directories and
default guest files on first launch; it is not a native module catalog.
Existing manifests are preserved when updating native binaries. Release staging uses `tools/prepare-release-rootfs.py --online` to place native
providers in `native/` and emit a Debian manifest with package URLs and hashes.
The runtime ZIP includes the entry points, app-local VC runtime DLLs, native
providers and manifest. Its built-in installer downloads and unpacks Debian
packages on first launch; no Python or external unpacker is needed. Debug builds
retain the offline `rootfs-seed/` layout.
The ZIP root contains only executable entry points, the manifest, runtime DLLs
and payload directories. License materials live in `licenses/`. User documentation
and build/validation reports stay outside the runtime ZIP;
launch `init.exe` or `worker.exe` directly without CMD wrappers.
Entry executables link Rust's standard
library statically; native modules retain their shared standard-library DLL.
There is no module catalog, image patching, heap redirection or runtime ELF facade.

Validation occurs in an owned staging directory before publishing files. Native
object sizes are queried through the module's borrowed-buffer C ABI; this loads
project DLLs and runs their normal initializers in the packaging process.
The read-only PE checks validate local code/data, forwarders and native imports.

Optional `--link-dir DIR` emits build-only ELF imports with standard symbol/version
tables outside the distribution. The build script uses `target/<profile>/elf-imports`.
No SDK directory or runtime ELF facade is published. The guest loader binds real
native addresses.

For a root that contains GCC or Clang, `tools/build.ps1 -Development` also writes
unversioned ELF link inputs such as `usr/lib/x86_64-linux-gnu/libc.so` inside
rootfs. They contain the native library's symbol/version tables and versioned
SONAME. GNU ld and Clang can therefore link Linux executables whose DT_NEEDED
selects the original PE implementation in `rootfs/lib`; these inputs do not add
a runtime dispatch layer. Prepare compiler packages before this final packaging
step, because Debian development packages also own the unversioned link paths.
Repackage with `-Development` after updating the native ABI. Ordinary packaging
does not install development inputs, and preserves existing rootfs files.

Each destination file is published through an owned temporary file and rename.
Unknown files are preserved; no recursive deletion or old ownership catalog is
used. Whole-distribution replacement is not atomic, and loaded DLLs must be
released before replacement.
