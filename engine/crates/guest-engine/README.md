# Guest engine

`kinakaze-guest-engine` is the native engine library used by the runtime.
The V2 worker authenticates with the public manager before calling `run` with
an explicit `GuestConfig`. The configuration owns the ELF provider registry,
executable host path, guest root, Linux working directory, argv and environment.
It never reads the worker's command line as guest arguments.

Private Rust allocations stay on the host heap. Memory published to guest code
uses explicit runtime allocations or registered mappings. The independent loader at
`libs/ld-linux-x86-64` serializes its own metadata and reconstructs private
collections around restored ELF mappings and fresh native library references. No DLL imports or standard-library allocator
are rewritten. This crate has no binary target or rootfs installer.

Process ELF files use a Linux initial stack and auxv, ELF TLS, FS/syscall
rewriting and the synchronous exception/JIT fallback before entering `_start`.
Missing strong imports fail linking before constructors. Static ELF uses the
same runtime-owned raw syscall, signal and mapping implementations as libc.
The independent ld.so owns GNU symbol lookup, TLS addresses, IFUNC resolution,
RTLD_NEXT, image introspection and on-demand DWARF unwinding. The engine publishes
the initial stack before relocation copies initial data into guest RELRO.

The engine calls the manager authority's image activation barrier after mapping,
TLS and descriptor restoration, before executable preinit, dependency
constructors and entry. The process authority must quiesce an old exec worker
before preparing a replacement: GNU IFUNC resolvers can execute during
relocation. Native fork/exec identity and worker activation remain runtime and
manager responsibilities; the engine transfers only its TLS/TEB/ABI transition state.

One `run` is permitted per native worker. A successfully entered process exits
through its Linux exit path; a returned status reports startup failure or a
freestanding ELF test result.

Run structural entry and launch-boundary tests with:

```powershell
./tools/build.ps1 -SkipFormat -DistDirectory artifacts/native-direct-dist
```

These tests cover entry classification, synchronous-fault classification and
launch argument validation. They do not substitute for V2 application launches.
