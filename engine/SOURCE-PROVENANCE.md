# Engine source provenance

The engine starts from this repository's existing self-developed compatibility
implementation. The source was copied on 2026-09-12; the original directories
remain intact.

| V2 location | Original location |
| --- | --- |
| `crates/kinakaze-abi` | `/crates/kinakaze-abi` |
| `crates/kinakaze-alloc` | `/crates/kinakaze-alloc` |
| `crates/kinakaze-runtime` | `/crates/kinakaze-runtime` |
| `crates/kinakaze-tls` | `/crates/kinakaze-tls` |
| `crates/kinakaze-vfs` | `/crates/kinakaze-vfs` |
| `crates/kinakaze-elf` | `/crates/kinakaze-elf` |
| `providers/libc` | `/libs/libc` |
| `providers/libpthread` | `/libs/libpthread` |
| `include` | `/include` |

These packages are Rust libraries consumed by the single V2 runtime DLL. The
provider DLLs expose guest ABI forwarding surfaces. Allocator, TLS, filesystem,
thread and native fork state therefore have one owner in each worker process.
The engine runtime dispatches directly to that owner; process entry hooks from
libc and the allocator resolve `kinakaze_runtime.dll` explicitly. Native image
identity and bootstrap lookups still refer to the actual worker executable.

The libc build script emits an export inventory instead of cdylib linker
arguments. The native fork and ELF machinery are preserved for integration;
copying source alone does not establish complete ABI compatibility. Validation
and supported application coverage are recorded by the V2 build and acceptance
tests.
