# Native images and Linux link inputs

`ModuleSet::discover` reads distribution filenames and PE export tables.
There is no module manifest or embedded descriptor. `ModuleImage` owns the
native library reference and resolves exact exported names and GNU version
aliases through the Windows loader. Data sizes and alignments come from the
module's bounded, borrowed-buffer `kinakaze_module_object_v1` query.

Function and data addresses remain valid while their image is retained. COPY
relocations explicitly coordinate shared data; native storage is not cloned.
The loader retains dependencies and registers lifecycle participants where
the corresponding fixed C ABI exists.

The link-input writer creates ordinary ELF SONAME, dynamic symbols and GNU
versions for Linux build tools. Data definitions have section storage, sizes
and alignments so GNU ld can resolve versioned data imports from other DSOs.
These files are used only when linking. Runtime execution uses the native
`.so` images and their storage directly, without an ELF facade or jump layer.
