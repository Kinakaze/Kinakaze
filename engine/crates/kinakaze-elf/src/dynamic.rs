//! The `PT_DYNAMIC` view of an ELF object.
//!
//! This is the only view a runtime loader may rely on. Section headers are a
//! link-time artifact: they are frequently stripped, they are never required to
//! be mapped, and `SHT_*` tables can describe data that does not exist at run
//! time. Everything the dynamic linker needs — the symbol table, the string
//! table, the hash tables, the relocations, the initializers — is reachable from
//! `PT_DYNAMIC` alone, which is exactly why the segment exists.
//!
//! Addresses inside the dynamic table are *virtual*, so they are translated back
//! to file offsets through the `PT_LOAD` segments. Reading from the file rather
//! than from the mapped image keeps parsing independent of how far relocation has
//! progressed, which matters because the tables have to be read *before* anything
//! can be relocated.

use crate::{ElfError, ElfFile, PT_DYNAMIC, PT_LOAD, ProgramHeader, i64_at, u64_at};

// Dynamic table tags. The values are ABI.
pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_PLTGOT: i64 = 3;
pub const DT_HASH: i64 = 4;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SYMENT: i64 = 11;
pub const DT_INIT: i64 = 12;
pub const DT_FINI: i64 = 13;
pub const DT_SONAME: i64 = 14;
pub const DT_RPATH: i64 = 15;
pub const DT_SYMBOLIC: i64 = 16;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_PLTREL: i64 = 20;
pub const DT_DEBUG: i64 = 21;
pub const DT_TEXTREL: i64 = 22;
pub const DT_JMPREL: i64 = 23;
pub const DT_BIND_NOW: i64 = 24;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_RUNPATH: i64 = 29;
pub const DT_FLAGS: i64 = 30;
pub const DT_PREINIT_ARRAY: i64 = 32;
pub const DT_PREINIT_ARRAYSZ: i64 = 33;
/// Relative relocations in the compressed `RELR` encoding.
pub const DT_RELR: i64 = 36;
pub const DT_RELRSZ: i64 = 35;
pub const DT_RELRENT: i64 = 37;

// GNU extensions, all of which modern toolchains emit by default.
pub const DT_GNU_HASH: i64 = 0x6fff_fef5;
pub const DT_VERSYM: i64 = 0x6fff_fff0;
pub const DT_RELACOUNT: i64 = 0x6fff_fff9;
pub const DT_FLAGS_1: i64 = 0x6fff_fffb;
pub const DT_VERDEF: i64 = 0x6fff_fffc;
pub const DT_VERDEFNUM: i64 = 0x6fff_fffd;
pub const DT_VERNEED: i64 = 0x6fff_fffe;
pub const DT_VERNEEDNUM: i64 = 0x6fff_ffff;

// `DT_FLAGS` bits.
pub const DF_ORIGIN: u64 = 0x01;
pub const DF_SYMBOLIC: u64 = 0x02;
pub const DF_TEXTREL: u64 = 0x04;
pub const DF_BIND_NOW: u64 = 0x08;
pub const DF_STATIC_TLS: u64 = 0x10;

// `DT_FLAGS_1` bits.
pub const DF_1_NOW: u64 = 0x0000_0001;
pub const DF_1_GLOBAL: u64 = 0x0000_0002;
pub const DF_1_NODELETE: u64 = 0x0000_0008;
pub const DF_1_INITFIRST: u64 = 0x0000_0020;
pub const DF_1_NOOPEN: u64 = 0x0000_0040;
pub const DF_1_ORIGIN: u64 = 0x0000_0080;
pub const DF_1_PIE: u64 = 0x0800_0000;

/// One `Elf64_Dyn` entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DynamicEntry {
    pub tag: i64,
    /// The union of `d_val` and `d_ptr`; which one it is depends on the tag.
    pub value: u64,
}

/// A located table: where it starts in the file and how big it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableLocation {
    /// Virtual address, as written in the dynamic table.
    pub address: u64,
    /// The same location translated to a file offset.
    pub file_offset: usize,
    /// Size in bytes, when the dynamic table states one.
    pub size: Option<u64>,
    /// Size of one entry, when the dynamic table states one.
    pub entry_size: Option<u64>,
}

impl TableLocation {
    /// Number of entries, when both sizes are known.
    pub fn count(self) -> Option<u64> {
        match (self.size, self.entry_size) {
            (Some(size), Some(entry)) if entry != 0 => Some(size / entry),
            _ => None,
        }
    }
}

/// The parsed `PT_DYNAMIC` segment.
///
/// Construction resolves every table this loader understands, so a caller that
/// gets a `DynamicInfo` can rely on the locations being in-bounds.
#[derive(Clone, Debug, Default)]
pub struct DynamicInfo {
    pub entries: Vec<DynamicEntry>,
    /// `DT_STRTAB`, which every other string reference is relative to.
    pub strings: Option<TableLocation>,
    /// `DT_SYMTAB`. Has no size of its own: the count comes from a hash table.
    pub symbols: Option<TableLocation>,
    pub sysv_hash: Option<TableLocation>,
    pub gnu_hash: Option<TableLocation>,
    /// `DT_RELA`, the object's own relocations.
    pub rela: Option<TableLocation>,
    /// `DT_JMPREL`, the PLT relocations.
    pub jmprel: Option<TableLocation>,
    /// Whether `DT_JMPREL` holds `RELA` (as x86_64 always does) or `REL`.
    pub plt_uses_rela: bool,
    /// `DT_RELR`, compressed relative relocations.
    pub relr: Option<TableLocation>,
    pub version_symbols: Option<TableLocation>,
    pub version_definitions: Option<TableLocation>,
    pub version_needs: Option<TableLocation>,
    pub version_definition_count: u64,
    pub version_need_count: u64,
    pub init: Option<u64>,
    pub fini: Option<u64>,
    pub init_array: Option<TableLocation>,
    pub fini_array: Option<TableLocation>,
    pub preinit_array: Option<TableLocation>,
    pub flags: u64,
    pub flags_1: u64,
    pub needed_offsets: Vec<u64>,
    pub soname_offset: Option<u64>,
    pub rpath_offset: Option<u64>,
    pub runpath_offset: Option<u64>,
    /// True when `DT_TEXTREL` or `DF_TEXTREL` is present, meaning relocations
    /// write into segments that are not writable at rest.
    pub text_relocations: bool,
    /// True when the object asks for all relocations to be resolved eagerly.
    pub bind_now: bool,
}

impl DynamicInfo {
    /// True when the object looks up its own definitions before the global scope.
    pub fn is_symbolic(&self) -> bool {
        self.flags & DF_SYMBOLIC != 0 || self.entries.iter().any(|entry| entry.tag == DT_SYMBOLIC)
    }

    /// True when the object may never be unloaded.
    pub fn is_nodelete(&self) -> bool {
        self.flags_1 & DF_1_NODELETE != 0
    }

    /// True when the object requires space in the static TLS block.
    pub fn needs_static_tls(&self) -> bool {
        self.flags & DF_STATIC_TLS != 0
    }
}

/// Translates virtual addresses to file offsets using the `PT_LOAD` segments.
///
/// The dynamic table stores virtual addresses, but the tables have to be read
/// before the image is relocated, so the file is the only available source.
#[derive(Clone, Debug)]
pub struct AddressTranslator {
    segments: Vec<ProgramHeader>,
}

impl AddressTranslator {
    pub fn new(headers: &[ProgramHeader]) -> Self {
        Self {
            segments: headers
                .iter()
                .copied()
                .filter(|header| header.kind == PT_LOAD && header.memory_size != 0)
                .collect(),
        }
    }

    /// Converts a virtual address to a file offset.
    ///
    /// Fails for an address in the `.bss` tail of a segment, which has no file
    /// backing. That is correct: no table the loader reads may live there.
    pub fn to_file_offset(&self, address: u64) -> Result<usize, ElfError> {
        for segment in &self.segments {
            let start = segment.virtual_address;
            let file_end = start
                .checked_add(segment.file_size)
                .ok_or(ElfError::IntegerOverflow)?;
            if address >= start && address < file_end {
                let delta = address - start;
                let offset = segment
                    .offset
                    .checked_add(delta)
                    .ok_or(ElfError::IntegerOverflow)?;
                return usize::try_from(offset).map_err(|_| ElfError::IntegerOverflow);
            }
        }
        Err(ElfError::UnmappedDynamicAddress)
    }

    /// True when the address falls inside a loadable segment, file-backed or not.
    pub fn is_loadable(&self, address: u64) -> bool {
        self.segments.iter().any(|segment| {
            let end = segment.virtual_address.saturating_add(segment.memory_size);
            address >= segment.virtual_address && address < end
        })
    }
}

impl<'a> ElfFile<'a> {
    /// Returns the `PT_DYNAMIC` segment, if the object has one.
    pub fn dynamic_segment(self) -> Result<Option<ProgramHeader>, ElfError> {
        Ok(self
            .program_headers()?
            .into_iter()
            .find(|header| header.kind == PT_DYNAMIC))
    }

    /// Parses `PT_DYNAMIC` and resolves every table it references.
    ///
    /// Returns `None` for a statically linked object, which has no dynamic
    /// segment and therefore needs no linking at all.
    pub fn dynamic_info(self) -> Result<Option<DynamicInfo>, ElfError> {
        let headers = self.program_headers()?;
        let Some(segment) = headers
            .iter()
            .copied()
            .find(|header| header.kind == PT_DYNAMIC)
        else {
            return Ok(None);
        };
        let translator = AddressTranslator::new(&headers);

        let start = usize::try_from(segment.offset).map_err(|_| ElfError::IntegerOverflow)?;
        let size = usize::try_from(segment.file_size).map_err(|_| ElfError::IntegerOverflow)?;
        let table = self
            .bytes()
            .get(start..start.checked_add(size).ok_or(ElfError::IntegerOverflow)?)
            .ok_or(ElfError::Truncated)?;

        let mut entries = Vec::new();
        // Entries are a fixed 16 bytes each; the table ends at DT_NULL even when
        // the segment is larger, which is common because linkers pad it.
        for chunk in table.chunks_exact(16) {
            let tag = i64_at(chunk, 0)?;
            let value = u64_at(chunk, 8)?;
            if tag == DT_NULL {
                break;
            }
            entries.push(DynamicEntry { tag, value });
        }

        let mut info = DynamicInfo {
            entries,
            ..DynamicInfo::default()
        };
        // x86_64 uses RELA everywhere; DT_PLTREL says which form DT_JMPREL takes.
        info.plt_uses_rela = true;

        // First pass: scalars and sizes, because the table locations below need
        // them and the dynamic table has no guaranteed ordering.
        let mut sizes = TableSizes::default();
        for entry in &info.entries {
            match entry.tag {
                DT_STRSZ => sizes.string_size = Some(entry.value),
                DT_SYMENT => sizes.symbol_entry = Some(entry.value),
                DT_RELASZ => sizes.rela_size = Some(entry.value),
                DT_RELAENT => sizes.rela_entry = Some(entry.value),
                DT_RELSZ => sizes.rel_size = Some(entry.value),
                DT_RELENT => sizes.rel_entry = Some(entry.value),
                DT_PLTRELSZ => sizes.plt_size = Some(entry.value),
                DT_RELRSZ => sizes.relr_size = Some(entry.value),
                DT_RELRENT => sizes.relr_entry = Some(entry.value),
                DT_INIT_ARRAYSZ => sizes.init_array_size = Some(entry.value),
                DT_FINI_ARRAYSZ => sizes.fini_array_size = Some(entry.value),
                DT_PREINIT_ARRAYSZ => sizes.preinit_array_size = Some(entry.value),
                DT_PLTREL => info.plt_uses_rela = entry.value as i64 == DT_RELA,
                DT_FLAGS => info.flags = entry.value,
                DT_FLAGS_1 => info.flags_1 = entry.value,
                DT_VERDEFNUM => info.version_definition_count = entry.value,
                DT_VERNEEDNUM => info.version_need_count = entry.value,
                DT_NEEDED => info.needed_offsets.push(entry.value),
                DT_SONAME => info.soname_offset = Some(entry.value),
                DT_RPATH => info.rpath_offset = Some(entry.value),
                DT_RUNPATH => info.runpath_offset = Some(entry.value),
                DT_TEXTREL => info.text_relocations = true,
                DT_BIND_NOW => info.bind_now = true,
                DT_INIT => info.init = Some(entry.value),
                DT_FINI => info.fini = Some(entry.value),
                _ => {}
            }
        }
        info.text_relocations |= info.flags & DF_TEXTREL != 0;
        info.bind_now |= info.flags & DF_BIND_NOW != 0 || info.flags_1 & DF_1_NOW != 0;

        // Second pass: the tables themselves.
        for entry in &info.entries {
            let locate = |size: Option<u64>, entry_size: Option<u64>| -> Result<_, ElfError> {
                Ok(TableLocation {
                    address: entry.value,
                    file_offset: translator.to_file_offset(entry.value)?,
                    size,
                    entry_size,
                })
            };
            match entry.tag {
                DT_STRTAB => info.strings = Some(locate(sizes.string_size, Some(1))?),
                // A symbol table has no size tag: its extent is implied by the
                // hash table, which is why one of the two must be present.
                DT_SYMTAB => {
                    info.symbols = Some(locate(None, Some(sizes.symbol_entry.unwrap_or(24)))?)
                }
                DT_HASH => info.sysv_hash = Some(locate(None, Some(4))?),
                DT_GNU_HASH => info.gnu_hash = Some(locate(None, None)?),
                DT_RELA => {
                    info.rela = Some(locate(
                        sizes.rela_size,
                        Some(sizes.rela_entry.unwrap_or(24)),
                    )?)
                }
                DT_REL => {
                    // 32-bit REL form. x86_64 objects do not use it, and silently
                    // ignoring it would corrupt memory, so it is rejected.
                    return Err(ElfError::UnsupportedRelFormat);
                }
                DT_JMPREL => {
                    let entry_size = if info.plt_uses_rela {
                        sizes.rela_entry.unwrap_or(24)
                    } else {
                        sizes.rel_entry.unwrap_or(16)
                    };
                    info.jmprel = Some(locate(sizes.plt_size, Some(entry_size))?);
                }
                DT_RELR => {
                    info.relr = Some(locate(
                        sizes.relr_size,
                        Some(sizes.relr_entry.unwrap_or(8)),
                    )?)
                }
                DT_VERSYM => info.version_symbols = Some(locate(None, Some(2))?),
                DT_VERDEF => info.version_definitions = Some(locate(None, None)?),
                DT_VERNEED => info.version_needs = Some(locate(None, None)?),
                DT_INIT_ARRAY => info.init_array = Some(locate(sizes.init_array_size, Some(8))?),
                DT_FINI_ARRAY => info.fini_array = Some(locate(sizes.fini_array_size, Some(8))?),
                DT_PREINIT_ARRAY => {
                    info.preinit_array = Some(locate(sizes.preinit_array_size, Some(8))?)
                }
                _ => {}
            }
        }

        // A dynamic object without a string table cannot name anything, which
        // means the table is corrupt rather than merely minimal.
        if info.symbols.is_some() && info.strings.is_none() {
            return Err(ElfError::MissingStringTable);
        }
        Ok(Some(info))
    }

    /// Reads a NUL-terminated string from the dynamic string table.
    pub fn dynamic_string(self, info: &DynamicInfo, offset: u64) -> Result<&'a str, ElfError> {
        let strings = info.strings.ok_or(ElfError::MissingStringTable)?;
        let base = strings.file_offset;
        let limit = match strings.size {
            Some(size) => usize::try_from(size).map_err(|_| ElfError::IntegerOverflow)?,
            None => self.bytes().len().saturating_sub(base),
        };
        let table = self
            .bytes()
            .get(base..base.checked_add(limit).ok_or(ElfError::IntegerOverflow)?)
            .ok_or(ElfError::Truncated)?;
        let offset = usize::try_from(offset).map_err(|_| ElfError::IntegerOverflow)?;
        crate::string_at(table, offset)
    }

    /// The `DT_NEEDED` list, in the order the linker recorded it.
    ///
    /// Order is load-bearing: it defines the breadth-first global symbol scope,
    /// so the first definition found along it wins.
    pub fn dynamic_needed(self, info: &DynamicInfo) -> Result<Vec<&'a str>, ElfError> {
        info.needed_offsets
            .iter()
            .map(|offset| self.dynamic_string(info, *offset))
            .collect()
    }

    /// The `DT_SONAME`, which is the name dependents refer to this object by.
    pub fn dynamic_soname(self, info: &DynamicInfo) -> Result<Option<&'a str>, ElfError> {
        match info.soname_offset {
            Some(offset) => Ok(Some(self.dynamic_string(info, offset)?)),
            None => Ok(None),
        }
    }

    /// `DT_RUNPATH`, or `DT_RPATH` when only the older tag is present.
    ///
    /// `DT_RUNPATH` takes precedence: when both exist, `DT_RPATH` is ignored, as
    /// the ABI requires.
    pub fn dynamic_search_path(self, info: &DynamicInfo) -> Result<Option<&'a str>, ElfError> {
        let offset = info.runpath_offset.or(info.rpath_offset);
        match offset {
            Some(offset) => Ok(Some(self.dynamic_string(info, offset)?)),
            None => Ok(None),
        }
    }
}

/// Size tags gathered before the tables that need them are located.
#[derive(Default)]
struct TableSizes {
    string_size: Option<u64>,
    symbol_entry: Option<u64>,
    rela_size: Option<u64>,
    rela_entry: Option<u64>,
    rel_size: Option<u64>,
    rel_entry: Option<u64>,
    plt_size: Option<u64>,
    relr_size: Option<u64>,
    relr_entry: Option<u64>,
    init_array_size: Option<u64>,
    fini_array_size: Option<u64>,
    preinit_array_size: Option<u64>,
}
