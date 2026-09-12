# Native packaging

```powershell
./tools/build.ps1 -DistDirectory artifacts/portable-dist
```

The packager reads standard PE exports and COFF import libraries. The import
library supplies each module's native filename. Original DLL bytes are copied
unchanged to `rootfs/lib/<name>.so`; required toolchain DLLs retain their original
names in the same directory. The distribution has three top-level entries:
`init.exe`, `worker.exe`, and `rootfs/`. Entry executables link Rust's standard
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
