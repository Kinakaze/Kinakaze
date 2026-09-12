# Native export generation

This build tool reads actual PE definitions, libc's linker aliases and reviewed
ELF version evidence. It updates each module's ordinary `exports.def` and its
borrowed-buffer object-layout query. It does not generate a module inventory.
Cargo files and linker LIBRARY directives are the build inputs.

```powershell
python tools/native-exports/generate.py --image-dir target/debug
python tools/native-exports/generate.py --image-dir target/debug --check
python tools/native-exports/audit.py --dist artifacts/native-direct-dist --observe artifacts/tool-root
```

Guest aliases use PRIVATE exports so Windows code continues to import the host
CRT's functions. Versioned names are exact PE exports (`name@VERSION`), not a
wildcard. `tools/abi/versions.tsv` and `elf-evidence.jsonl` retain observed ABI
requirements and source hashes for reproducible generation; neither is loaded
at runtime. Object layouts describe actual Linux ABI payloads, not Rust wrapper
sizes. Unknown object layouts are rejected rather than guessed.

The remaining forwarding module exports are visible in their `.def` inputs;
source ownership must be split before those aliases can be removed.
