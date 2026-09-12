# CUDA driver bridge

`libcuda.so.1` / `kinakaze_libcuda.dll` forward Linux x86-64 CUDA Driver API calls to
the system Windows `nvcuda.dll`. The implementation links once into the shared
runtime. No NVIDIA DLL is loaded, device queried, context created or metrics
collected merely because the provider is present.

The installed NVIDIA GPU performs the work. PTX is JIT-compiled by the real driver;
cubin produced by NVIDIA `ptxas` is loaded as compiled GPU code. This is a process
ABI bridge, not PCI device assignment to a VM.

## Module boundaries

| File | Responsibility |
| --- | --- |
| `api.rs` | 106 typed entry points: initialization/device queries, contexts, device/pinned/managed memory, copies and fills, streams, events, modules and launches |
| `host.rs` | System-directory DLL loading, cached host exports, versioned CUDA queries, guest TLS restoration |
| `dispatch.rs` | Both `cuGetProcAddress` ABIs; version/stream-aware selection of guest-callable thunks |
| `module.rs` | `cuModuleLoad` reads the guest VFS, including tmpfs/mounted files, and calls the driver's memory-image loader |
| `lifecycle.rs` | Fork state: an initialized CUDA process cannot give its native handles to a fork child; fresh exec is supported |

Ordinary calls use a cached native pointer and a compiled SysV-to-Windows ABI
thunk. There is no per-call heap allocation, handle-map lookup, telemetry timer or
background worker in this provider. File-based module loading owns a temporary
buffer and reserves the known file size once. Driver resources are released by
the normal CUDA destruction/free APIs; process death is owned by the native
driver and the worker Job lifecycle.

CUDA's entry-point query returns internal implementation addresses, not the
public PE export addresses. Each reviewed signature has a CUDA-version anchor
from NVIDIA's `cudaTypedefs.h`. Dispatch compares the requested host address with
that anchor, then returns the corresponding SysV thunk. It never passes an
untranslated Windows function pointer to the guest. PTDS/PTSZ entry points use
their own host exports. Unsupported bridge entries report the documented missing
symbol result; the old query ABI returns `CUDA_ERROR_NOT_FOUND`, while v2 returns
success with a null pointer and query status.

References: [NVIDIA driver entry-point access](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__DRIVER__ENTRY__POINT.html),
[CUDA 11.8 query ABI](https://docs.nvidia.com/cuda/archive/11.8.0/cuda-driver-api/group__CUDA__DRIVER__ENTRY__POINT.html),
[CUDA execution control](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__EXEC.html),
[PTX ISA](https://docs.nvidia.com/cuda/parallel-thread-execution/index.html).

## Verification

Run from the V2 workspace root:

```powershell
./tools/build.ps1
python tests/guest/run-cuda.py --fork
python tools/cuda/prepare-sdk.py --compiler
python tools/cuda/verify-abi.py
python tests/guest/run-cuda.py --ptxas artifacts/cuda-sdk/ptxas.exe --gpu-architecture sm_89
```

`prepare-sdk.py` downloads checksum-pinned official NVIDIA wheels without
installing them into Python or changing system CUDA. The headers are optional
verification inputs; normal provider builds and the PTX test do not need a CUDA
SDK. Keep `--gpu-architecture` appropriate to the device being tested. The current
hardware result is an RTX 4060 Laptop GPU (sm_89), NVIDIA driver 596.49, CUDA driver
API version 13020.

The freestanding Linux ELF probe checks direct and queried calls, the old and new
query ABIs, unsupported/future/old incompatible API requests, file errors, pinned
host memory, async uploads/downloads, GPU fill, and **independent** direct and
PTDS kernel launches. Both launches compute and verify every element of a
1025-element vector, including a partial last block. Streams/events/modules and
memory are freed, then the current context is verified absent. The fork probe
checks initialization in a child before parent initialization, rejection of
inherited CUDA state, and a fresh CUDA compute run after exec. Reports record
worker/runtime/source/PTX hashes and, for AOT, compiler/cubin hashes.

```powershell
cargo test --manifest-path engine/Cargo.toml -p kinakaze-libcuda --lib
```

The unit test verifies that merely loading the provider leaves `nvcuda.dll`
unloaded. The explicit GPU probe fails if hardware or any required behavior is
unavailable; absence of a GPU is not counted as successful GPU verification.

## Remaining surface

Driver API compute works in the probes above. Full Linux `libcudart`, PyTorch,
cuBLAS/cuDNN/NCCL, host callbacks, graphs, arrays/textures, IPC/external-memory
handles, CUDA export tables and the newer context-create parameter structures
still need dedicated bridges and end-to-end tests. No capability is inferred
from an export count. Unsupported queried entries stay unavailable instead of
being forwarded with a guessed signature or foreign callback layout.

After successful `cuInit`, fork preserves CPU execution but CUDA operations in
that child return `CUDA_ERROR_NOT_SUPPORTED`; exec establishes a fresh process
and driver state. The provider does not duplicate GPU allocations or promise
CUDA-context inheritance across fork.
