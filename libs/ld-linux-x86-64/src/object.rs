//! One mapped ELF object and the metadata the linker keeps about it.

use std::path::{Path, PathBuf};

use kinakaze_elf::dynamic::{DF_1_PIE, DynamicInfo};
use kinakaze_elf::hash::{GnuHash, SysvHash, gnu_hash_name, sysv_hash_name};
use kinakaze_elf::version::VersionTable;
use kinakaze_elf::{
    DynamicSymbol, ElfFile, PF_R, PF_W, PF_X, PT_GNU_RELRO, PT_INTERP, PT_LOAD, ProgramHeader,
    STT_GNU_IFUNC, STT_TLS,
};

use crate::LinkError;
pub(crate) mod mapping;
use mapping::ImageMapping;

/// Index of an object in the linker's load order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectId(pub usize);

/// Page size assumed for segment alignment.
///
/// x86_64 Linux images are built for 4 KiB pages, and the ELF segments are
/// aligned to that regardless of what Windows reports for allocation granularity.
pub const PAGE_SIZE: u64 = 4096;

/// A shared object mapped into this process.
pub struct MappedObject {
    /// The name dependents refer to this object by: its `DT_SONAME`, or the file
    /// name when it has none.
    pub name: String,
    pub path: PathBuf,
    /// The file image, retained because every table is read from it rather than
    /// from memory. Parsing must work before relocation has run.
    pub bytes: crate::ImmutableBytes,
    /// Base of the reservation this object was mapped into.
    pub allocation: *mut u8,
    pub allocation_len: usize,
    /// Owns the initialized section and its private view through unload/fork.
    pub(crate) _mapping: ImageMapping,
    /// Difference between the mapped address and the object's link-time address.
    pub load_bias: i128,
    pub dynamic: DynamicInfo,
    pub versions: VersionTable,
    /// Number of entries in `DT_SYMTAB`, derived from whichever hash table exists.
    pub symbol_count: u32,
    /// The TLS module id assigned when this object has a `PT_TLS` segment.
    pub tls_module: Option<usize>,
    /// `DT_NEEDED`, resolved to the objects that satisfied each entry.
    pub dependencies: Vec<ObjectId>,
    /// Set once the initializers have run, so they never run twice.
    pub initialized: bool,
    /// Set once the process loader has run this object's `DT_PREINIT_ARRAY`.
    /// Only the main executable may have one, and it runs separately before
    /// dependency constructors.
    pub preinitialized: bool,
    /// `dlopen` reference count. The root object and its dependencies start at 1.
    pub references: usize,
    /// True when this object may not be unloaded.
    pub nodelete: bool,
    /// True for the object that owns the process entry point.
    pub is_executable: bool,
    /// True when the Linux kernel would enter this image without a `PT_INTERP`.
    ///
    /// Its startup code owns static-PIE relocation, TLS bootstrap and RELRO. The
    /// hosted linker must map and patch raw syscalls, but must not impersonate a
    /// dynamic interpreter that the ELF explicitly does not name.
    pub self_relocating: bool,
}

// The raw pointer is an owned private section view, not shared writable state, so the
// object is safe to move between threads. The linker serializes access itself.
unsafe impl Send for MappedObject {}

impl MappedObject {
    /// Reads and maps an object, leaving relocation to the caller.
    pub fn map(path: &Path, requested_name: Option<&str>) -> Result<Self, LinkError> {
        let snapshot = crate::linker::profile::begin("image-snapshot", path.display());
        let bytes = crate::snapshot_guest_image(path)?;
        drop(snapshot);
        Self::map_image(path, requested_name, bytes)
    }

    /// Maps an object from a stable executable snapshot supplied by exec handoff.
    pub fn map_image(
        path: &Path,
        requested_name: Option<&str>,
        bytes: crate::ImmutableBytes,
    ) -> Result<Self, LinkError> {
        let display = path.display().to_string();
        let elf = ElfFile::parse(&bytes)?;
        let headers = elf.program_headers()?;
        let span = elf.load_span(PAGE_SIZE)?;
        let is_executable = elf.header().object_type == kinakaze_elf::ET_EXEC;
        // Fixed executables need a 64 KiB-aligned view base. Dynamic objects
        // retain the ELF page span and can be placed anywhere.
        let (span_start, allocation_len) = if is_executable {
            let start = span.start & !65535;
            let end = align_up(span.end, 65536)?;
            (
                start,
                usize::try_from(end - start).map_err(|_| LinkError::AddressOverflow)?,
            )
        } else {
            (span.start, span.len()?)
        };
        let mapping = ImageMapping::new(
            is_executable,
            span_start,
            allocation_len,
            &display,
            &bytes,
            &headers,
        )?;
        let allocation = mapping.base;
        let load_bias = allocation as i128 - span_start as i128;
        // The owner releases both view and retained handle on any parse error.
        let staged = (|| -> Result<_, LinkError> {
            let dynamic = elf
                .dynamic_info()?
                // A statically linked object has no dynamic segment. It needs no
                // linking, so an empty table is the correct description of it.
                .unwrap_or_default();
            let self_relocating = !headers.iter().any(|header| header.kind == PT_INTERP)
                && (elf.header().object_type == kinakaze_elf::ET_EXEC
                    || dynamic.flags_1 & DF_1_PIE != 0);
            let versions = VersionTable::parse(elf, &dynamic)?;
            let symbol_count = count_symbols(elf, &dynamic)?;

            let tls_module = if self_relocating {
                None
            } else {
                match elf.tls_segment()? {
                    Some(tls) => Some(register_tls(&bytes, tls, &display)?),
                    None => None,
                }
            };

            let name = resolve_name(elf, &dynamic, path, requested_name)?;
            Ok((
                dynamic,
                versions,
                symbol_count,
                tls_module,
                name,
                self_relocating,
            ))
        })();

        match staged {
            Ok((dynamic, versions, symbol_count, tls_module, name, self_relocating)) => {
                let nodelete = dynamic.is_nodelete();
                Ok(Self {
                    name,
                    path: path.to_path_buf(),
                    bytes,
                    allocation,
                    allocation_len,
                    _mapping: mapping,
                    load_bias,
                    dynamic,
                    versions,
                    symbol_count,
                    tls_module,
                    dependencies: Vec::new(),
                    initialized: false,
                    preinitialized: false,
                    references: 1,
                    nodelete,
                    is_executable,
                    self_relocating,
                })
            }
            Err(error) => Err(error),
        }
    }

    /// Parses the retained file image.
    pub fn elf(&self) -> Result<ElfFile<'_>, LinkError> {
        Ok(ElfFile::parse(&self.bytes)?)
    }

    pub(crate) fn publish_relocated_tls(&self) -> Result<(), LinkError> {
        let Some(module) = self.tls_module else {
            return Ok(());
        };
        let invalid = || LinkError::InvalidTls {
            object: self.name.clone(),
        };
        let segment = self.elf()?.tls_segment()?.ok_or_else(invalid)?;
        let start = self.resolve_address(segment.virtual_address)?;
        let length = usize::try_from(segment.file_size).map_err(|_| invalid())?;
        let end = start.checked_add(length).ok_or_else(invalid)?;
        let mapped_end = (self.allocation as usize)
            .checked_add(self.allocation_len)
            .ok_or_else(invalid)?;
        if start < self.allocation as usize || end > mapped_end {
            return Err(invalid());
        }
        // Relocations have finished and the initialized TLS segment is inside
        // the object's live mapping. .tbss has no file image and stays zero.
        let image = unsafe { core::slice::from_raw_parts(start as *const u8, length) };
        kinakaze_tls::relocate_elf_module_image(module, image).map_err(|_| invalid())
    }

    /// Translates a link-time virtual address to its mapped address.
    pub fn resolve_address(&self, virtual_address: u64) -> Result<usize, LinkError> {
        let value = self
            .load_bias
            .checked_add(virtual_address as i128)
            .ok_or(LinkError::AddressOverflow)?;
        usize::try_from(value).map_err(|_| LinkError::AddressOverflow)
    }

    /// Reads one entry from `DT_SYMTAB`.
    ///
    /// Symbols are read from the file rather than the mapped image so that lookup
    /// works before the image is relocated.
    pub fn symbol(&self, index: u32) -> Result<Option<DynamicSymbol<'_>>, LinkError> {
        let Some(symbols) = self.dynamic.symbols else {
            return Ok(None);
        };
        if index >= self.symbol_count {
            return Ok(None);
        }
        let elf = self.elf()?;
        let entry_size = symbols.entry_size.unwrap_or(24) as usize;
        let offset = symbols
            .file_offset
            .checked_add(index as usize * entry_size)
            .ok_or(LinkError::AddressOverflow)?;
        let entry = elf.slice(offset, 24)?;
        let name_offset = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let name = elf.dynamic_string(&self.dynamic, u64::from(name_offset))?;
        Ok(Some(DynamicSymbol {
            index,
            name,
            info: entry[4],
            other: entry[5],
            section_index: u16::from_le_bytes([entry[6], entry[7]]),
            value: u64::from_le_bytes([
                entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14],
                entry[15],
            ]),
            size: u64::from_le_bytes([
                entry[16], entry[17], entry[18], entry[19], entry[20], entry[21], entry[22],
                entry[23],
            ]),
        }))
    }

    /// Finds a defined symbol by name, using whichever hash table exists.
    ///
    pub fn lookup(&self, name: &str) -> Result<Option<DynamicSymbol<'_>>, LinkError> {
        self.lookup_versioned(name, None)
    }

    /// Finds a defined symbol by name and version, using whichever hash table exists.
    pub fn lookup_versioned(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<DynamicSymbol<'_>>, LinkError> {
        if self.dynamic.symbols.is_none() {
            return Ok(None);
        }
        let elf = self.elf()?;

        // GNU hash first: it is what modern toolchains emit, and its Bloom filter
        // rejects most misses without touching the chain.
        if let Some(location) = self.dynamic.gnu_hash
            && let Ok(table) = GnuHash::parse(elf, location)
        {
            let hash = gnu_hash_name(name);
            if !table.may_contain(hash) {
                return Ok(None);
            }
            let mut matched: Option<DynamicSymbol<'_>> = None;
            let mut fallback: Option<DynamicSymbol<'_>> = None;
            table.for_each_candidate::<LinkError, _>(hash, |index| {
                if let Some(symbol) = self.matching_definition(index, name)? {
                    if crate::scope::version_matches(self, symbol, version) {
                        matched = Some(symbol);
                        return Ok(true);
                    }
                    if fallback.is_none() && version.is_none() {
                        fallback = Some(symbol);
                    }
                }
                Ok(false)
            })?;
            return Ok(matched.or(fallback));
        }

        if let Some(location) = self.dynamic.sysv_hash
            && let Ok(table) = SysvHash::parse(elf, location)
        {
            let hash = sysv_hash_name(name);
            let mut matched: Option<DynamicSymbol<'_>> = None;
            let mut fallback: Option<DynamicSymbol<'_>> = None;
            table.for_each_candidate::<LinkError, _>(hash, |index| {
                if let Some(symbol) = self.matching_definition(index, name)? {
                    if crate::scope::version_matches(self, symbol, version) {
                        matched = Some(symbol);
                        return Ok(true);
                    }
                    if fallback.is_none() && version.is_none() {
                        fallback = Some(symbol);
                    }
                }
                Ok(false)
            })?;
            return Ok(matched.or(fallback));
        }

        // No hash table. Linear scan is the only option, and it is correct if
        // slow, so a hand-built object without one still links.
        let mut fallback: Option<DynamicSymbol<'_>> = None;
        for index in 0..self.symbol_count {
            if let Some(symbol) = self.matching_definition(index, name)? {
                if crate::scope::version_matches(self, symbol, version) {
                    return Ok(Some(symbol));
                }
                if fallback.is_none() && version.is_none() {
                    fallback = Some(symbol);
                }
            }
        }
        Ok(fallback)
    }

    /// Returns the closest dynamic definition at or below an address in this
    /// image, which is the symbol portion of Linux `dladdr`.
    pub fn symbol_for_address(&self, address: usize) -> Result<Option<(String, usize)>, LinkError> {
        let mut best: Option<(String, usize)> = None;
        for index in 0..self.symbol_count {
            let Some(symbol) = self.symbol(index)? else {
                continue;
            };
            if !symbol.is_defined() || symbol.is_absolute() || symbol.name.is_empty() {
                continue;
            }
            let start = self.resolve_address(symbol.value)?;
            if start > address {
                continue;
            }
            if symbol.size != 0 && address >= start.saturating_add(symbol.size as usize) {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(_, candidate)| start > *candidate)
            {
                best = Some((symbol.name.to_owned(), start));
            }
        }
        Ok(best)
    }

    /// Confirms a candidate index really defines `name`.
    fn matching_definition(
        &self,
        index: u32,
        name: &str,
    ) -> Result<Option<DynamicSymbol<'_>>, LinkError> {
        let Some(symbol) = self.symbol(index)? else {
            return Ok(None);
        };
        if symbol.name != name || !symbol.is_defined() {
            return Ok(None);
        }
        // A hidden or internal definition is not visible to other objects.
        if symbol.is_local_only() {
            return Ok(None);
        }
        Ok(Some(symbol))
    }

    /// The version name attached to a definition, if it carries one.
    pub fn definition_version(&self, index: u32) -> Option<&str> {
        let elf = ElfFile::parse(&self.bytes).ok()?;
        self.versions.definition_name(elf, index)
    }

    /// True when a definition is hidden, meaning an unversioned reference must
    /// not bind to it.
    pub fn definition_is_hidden(&self, index: u32) -> bool {
        match ElfFile::parse(&self.bytes) {
            Ok(elf) => self.versions.is_hidden(elf, index),
            Err(_) => false,
        }
    }

    /// The version a reference demands, if it demands one.
    pub fn requirement_version(&self, index: u32) -> Option<&str> {
        let elf = ElfFile::parse(&self.bytes).ok()?;
        self.versions.requirement_name(elf, index)
    }

    /// The address a symbol defined by this object resolves to.
    ///
    /// `STT_GNU_IFUNC` is not resolved here: the resolver has to run after the
    /// object is relocated and protected, so the caller decides when.
    pub fn definition_address(&self, symbol: DynamicSymbol<'_>) -> Result<usize, LinkError> {
        if symbol.is_absolute() {
            // An absolute symbol is a value, not a location; adding the bias
            // would corrupt it.
            return usize::try_from(symbol.value).map_err(|_| LinkError::AddressOverflow);
        }
        self.resolve_address(symbol.value)
    }

    /// Applies the final page permissions from the `PT_LOAD` flags.
    ///
    /// Deliberately ordered so that overlapping pages end up with the union of
    /// what the segments need: two segments can share a page, and applying them
    /// blindly in order would drop write permission that a later segment needs, or
    /// grant execute to a page that only holds data.
    pub fn protect(&self) -> Result<(), LinkError> {
        let elf = self.elf()?;
        let mut loads = Vec::new();
        let mut boundaries = Vec::new();
        for segment in elf.program_headers()? {
            if segment.kind != PT_LOAD || segment.memory_size == 0 {
                continue;
            }
            let start = segment.virtual_address & !(PAGE_SIZE - 1);
            let segment_end = segment
                .virtual_address
                .checked_add(segment.memory_size)
                .ok_or(LinkError::AddressOverflow)?;
            let end = align_up(segment_end, PAGE_SIZE)?;
            loads.push((start, end, segment.flags));
            boundaries.push(start);
            boundaries.push(end);
        }

        // PT_LOAD segments are few, while a large DSO can span thousands of
        // pages. Sweep only their aligned boundaries and OR overlapping segment
        // flags per interval instead of inserting every page into a BTreeMap.
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut run: Option<(u64, u64, u32)> = None;
        for window in boundaries.windows(2) {
            let start = window[0];
            let end = window[1];
            let flags = loads
                .iter()
                .filter(|(load_start, load_end, _)| *load_start < end && *load_end > start)
                .fold(0, |wanted, (_, _, flags)| wanted | flags);
            if flags == 0 {
                if let Some((run_start, run_end, run_flags)) = run.take() {
                    self.apply_protection_range(run_start, run_end, run_flags)?;
                }
                continue;
            }
            match run {
                Some((run_start, run_end, current)) if run_end == start && current == flags => {
                    run = Some((run_start, end, current));
                }
                Some((run_start, run_end, current)) => {
                    self.apply_protection_range(run_start, run_end, current)?;
                    run = Some((start, end, flags));
                }
                None => run = Some((start, end, flags)),
            }
        }
        if let Some((start, end, flags)) = run {
            self.apply_protection_range(start, end, flags)?;
        }
        Ok(())
    }

    fn apply_protection_range(&self, start: u64, end: u64, flags: u32) -> Result<(), LinkError> {
        let address = self.resolve_address(start)?;
        let length = usize::try_from(end - start).map_err(|_| LinkError::AddressOverflow)?;
        apply_protection(address, length, flags, &self.name)
    }

    /// Makes the `PT_GNU_RELRO` region read-only.
    ///
    /// Must run after relocation, because the relocations write into it: the whole
    /// purpose of RELRO is that the GOT and other relocated data become immutable
    /// once the linker is finished with them.
    pub fn apply_relro(&self) -> Result<(), LinkError> {
        if self.self_relocating {
            return Ok(());
        }
        let elf = self.elf()?;
        let Some(relro) = elf
            .program_headers()?
            .into_iter()
            .find(|header| header.kind == PT_GNU_RELRO)
        else {
            return Ok(());
        };
        if relro.memory_size == 0 {
            return Ok(());
        }
        let start = relro.virtual_address & !(PAGE_SIZE - 1);
        // Deliberately rounds the end DOWN. A partial page at the tail is shared
        // with data that stays writable, and protecting it would fault on the
        // first ordinary write to that neighbour.
        let end = (relro.virtual_address + relro.memory_size) & !(PAGE_SIZE - 1);
        if end <= start {
            return Ok(());
        }
        let address = self.resolve_address(start)?;
        apply_protection(address, (end - start) as usize, PF_R, &self.name)
    }

    /// The mapped entry point, validated to lie inside this image.
    pub fn entry(&self) -> Result<usize, LinkError> {
        let entry = self.elf()?.header().entry;
        if entry == 0 {
            return Err(LinkError::NoEntryPoint {
                object: self.name.clone(),
            });
        }
        let entry = self.resolve_address(entry)?;
        let start = self.allocation as usize;
        let end = start
            .checked_add(self.allocation_len)
            .ok_or(LinkError::AddressOverflow)?;
        if (start..end).contains(&entry) {
            Ok(entry)
        } else {
            Err(LinkError::NoEntryPoint {
                object: self.name.clone(),
            })
        }
    }

    /// True when this symbol index names an indirect function.
    pub fn is_ifunc(&self, symbol: DynamicSymbol<'_>) -> bool {
        symbol.symbol_type() == STT_GNU_IFUNC
    }

    /// True when the symbol is thread-local, so it has an offset rather than an
    /// address.
    pub fn is_tls(&self, symbol: DynamicSymbol<'_>) -> bool {
        symbol.symbol_type() == STT_TLS
    }
}

/// Derives the symbol table length from whichever hash table the object has.
///
/// `DT_SYMTAB` carries no count of its own, so without a hash table there is no
/// way to know where the table ends. The fallback treats the gap to the next known
/// table as the extent, which is what tools do when a hash table is absent.
fn count_symbols(elf: ElfFile<'_>, info: &DynamicInfo) -> Result<u32, LinkError> {
    let Some(symbols) = info.symbols else {
        return Ok(0);
    };
    if let Some(location) = info.gnu_hash
        && let Ok(table) = GnuHash::parse(elf, location)
        && let Ok(count) = table.symbol_count()
    {
        return Ok(count);
    }
    if let Some(location) = info.sysv_hash
        && let Ok(table) = SysvHash::parse(elf, location)
    {
        return Ok(table.symbol_count());
    }
    // No hash table: bound the table by the nearest following table in the file,
    // which is conservative but never reads past the end.
    let entry_size = symbols.entry_size.unwrap_or(24) as usize;
    let start = symbols.file_offset;
    let limit = [
        info.strings.map(|table| table.file_offset),
        info.rela.map(|table| table.file_offset),
        info.jmprel.map(|table| table.file_offset),
        info.version_symbols.map(|table| table.file_offset),
    ]
    .into_iter()
    .flatten()
    .filter(|offset| *offset > start)
    .min()
    .unwrap_or(elf.bytes().len());
    Ok(((limit.saturating_sub(start)) / entry_size.max(1)) as u32)
}

/// Chooses the name this object is known by in the scope.
fn resolve_name(
    elf: ElfFile<'_>,
    info: &DynamicInfo,
    path: &Path,
    requested: Option<&str>,
) -> Result<String, LinkError> {
    // DT_SONAME is authoritative: it is the name dependents recorded.
    if let Some(soname) = elf.dynamic_soname(info)? {
        return Ok(soname.to_owned());
    }
    if let Some(requested) = requested {
        return Ok(requested.to_owned());
    }
    Ok(path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string()))
}

fn align_up(value: u64, align: u64) -> Result<u64, LinkError> {
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or(LinkError::AddressOverflow)
}

#[cfg(windows)]
fn apply_protection(address: usize, len: usize, flags: u32, object: &str) -> Result<(), LinkError> {
    use kinakaze_runtime::memory_protection::protect_preserving_copy_on_write as VirtualProtect;
    let mut previous = 0u32;
    // SAFETY: the range lies inside this object's own reservation.
    let ok = unsafe {
        VirtualProtect(
            address as *mut core::ffi::c_void,
            len,
            windows_protection(flags),
            &mut previous,
        )
    };
    if ok == 0 {
        return Err(LinkError::ProtectionFailed {
            object: object.to_owned(),
        });
    }
    Ok(())
}

#[cfg(windows)]
fn windows_protection(flags: u32) -> u32 {
    use windows_sys::Win32::System::Memory::{
        PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_READONLY,
        PAGE_READWRITE,
    };
    match (flags & PF_R != 0, flags & PF_W != 0, flags & PF_X != 0) {
        (false, false, false) => PAGE_NOACCESS,
        (true, false, false) => PAGE_READONLY,
        (_, true, false) => PAGE_READWRITE,
        (false, false, true) => PAGE_EXECUTE,
        (true, false, true) => PAGE_EXECUTE_READ,
        (_, true, true) => PAGE_EXECUTE_READWRITE,
    }
}

#[cfg(not(windows))]
fn apply_protection(_: usize, _: usize, _: u32, object: &str) -> Result<(), LinkError> {
    Err(LinkError::ProtectionFailed {
        object: object.to_owned(),
    })
}

/// Copies every `PT_LOAD` segment's file contents into the mapping.
///
/// The `.bss` tail needs no work: a new pagefile section contains zeroes.
fn copy_segments(
    bytes: &[u8],
    headers: &[ProgramHeader],
    allocation: usize,
    span_start: u64,
    allocation_len: usize,
) -> Result<(), LinkError> {
    for segment in headers {
        if segment.kind != PT_LOAD || segment.file_size == 0 {
            continue;
        }
        let start = usize::try_from(segment.offset).map_err(|_| LinkError::AddressOverflow)?;
        let len = usize::try_from(segment.file_size).map_err(|_| LinkError::AddressOverflow)?;
        let end = start.checked_add(len).ok_or(LinkError::AddressOverflow)?;
        let source = bytes.get(start..end).ok_or(LinkError::AddressOverflow)?;
        let offset = segment
            .virtual_address
            .checked_sub(span_start)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(LinkError::AddressOverflow)?;
        if segment.file_size > segment.memory_size
            || offset
                .checked_add(len)
                .is_none_or(|end| end > allocation_len)
        {
            return Err(LinkError::AddressOverflow);
        }
        let target = allocation
            .checked_add(offset)
            .ok_or(LinkError::AddressOverflow)? as *mut u8;
        // SAFETY: the parser bounds-checked `source`, and the destination lies
        // inside the reservation sized from the object's own load span.
        unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), target, len) };
    }
    Ok(())
}

/// Registers a `PT_TLS` segment as a TLS module.
fn register_tls(bytes: &[u8], tls: ProgramHeader, display: &str) -> Result<usize, LinkError> {
    let invalid = || LinkError::InvalidTls {
        object: display.to_owned(),
    };
    let start = usize::try_from(tls.offset).map_err(|_| invalid())?;
    let file_size = usize::try_from(tls.file_size).map_err(|_| invalid())?;
    let end = start.checked_add(file_size).ok_or_else(invalid)?;
    let memory_size = usize::try_from(tls.memory_size).map_err(|_| invalid())?;
    let align = usize::try_from(tls.align.max(1)).map_err(|_| invalid())?;
    let image = bytes.get(start..end).ok_or_else(invalid)?;
    kinakaze_tls::register_elf_module(image, memory_size, align).map_err(|_| invalid())
}
