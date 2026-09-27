# Native images and Linux link inputs

`ModuleSet::discover` reads distribution filenames and PE export tables.
There is no module manifest or embedded descriptor. `ModuleImage` owns the
native library reference and resolves exact exported names and GNU version
aliases through the Windows loader. Data sizes and alignments come from the
module's bounded, borrowed-buffer `kinakaze_module_object_v1` query.

Ordinary workers use `ModuleCatalog` to validate every PE export table and
module identity, retain the inspected read-only files, and register provider
factories. Discovery neither loads layout-query DLLs nor constructs guest
declarations for unused providers. An actual dependency or `dlopen` constructs
and validates its provider's declarations and queries object layouts;
relocation or explicit symbol lookup then resolves only the requested export.
There is no pre-bound address table or cross-process address cache. Required
lifecycle callbacks still run in their original initialization order, and
`RTLD_NOLOAD` inspects registration without invoking a factory. `dladdr` may
inspect exports while finding an address, but does not publish incidental
symbol bindings. `ModuleImage` retains its eager utility API for explicit
whole-module inspection. `ModuleSet::discover` also retains its eager, fully
validated metadata API for packaging and explicit inspection.

Pending providers pin the exact files whose PE tables were validated, so
their contents cannot change before first use. Module-specific declaration,
layout, native load and symbol errors are returned at the corresponding
dependency/lookup operation; they never
silently fall back to another ELF file. Fork restore reconstructs its own
registry and materializes the providers recorded in the parent's live scope.
The first layout-query load, binding and fork registration share one mapping
transaction; the query's DLL owner is transferred directly to the provider.

Function and data addresses remain valid while their image is retained. COPY
relocations explicitly coordinate shared data; native storage is not cloned.
The loader retains dependencies and registers lifecycle participants where
the corresponding fixed C ABI exists.

The link-input writer creates ordinary ELF SONAME, dynamic symbols and GNU
versions for Linux build tools. Data definitions have section storage, sizes
and alignments so GNU ld can resolve versioned data imports from other DSOs.
These files are used only when linking. Runtime execution uses the native
`.so` images and their storage directly, without an ELF facade or jump layer.
