//! Bounds-checked ELF64/x86_64 parser used by the Windows loader.

use core::fmt;

pub mod dynamic;
pub mod hash;
pub mod version;

pub use dynamic::{AddressTranslator, DynamicEntry, DynamicInfo, TableLocation};

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 62;

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_TLS: u32 = 7;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const SHT_RELA: u32 = 4;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_DYNSYM: u32 = 11;

pub const PT_GNU_RELRO: u32 = 0x6474_e552;
pub const PT_GNU_STACK: u32 = 0x6474_e551;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;

pub const SHN_UNDEF: u16 = 0;
/// A symbol whose value is absolute and must not be adjusted by the load bias.
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;

// Symbol types.
pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;
pub const STT_COMMON: u8 = 5;
pub const STT_TLS: u8 = 6;
/// An indirect function: the "address" is a resolver to call at load time.
pub const STT_GNU_IFUNC: u8 = 10;

// Symbol bindings.
pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STB_GNU_UNIQUE: u8 = 10;

// Symbol visibility, held in the low two bits of `st_other`.
pub const STV_DEFAULT: u8 = 0;
pub const STV_INTERNAL: u8 = 1;
pub const STV_HIDDEN: u8 = 2;
pub const STV_PROTECTED: u8 = 3;

pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_SONAME: i64 = 14;

// x86_64 relocation types, from the psABI.
pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_GOT32: u32 = 3;
pub const R_X86_64_PLT32: u32 = 4;
pub const R_X86_64_COPY: u32 = 5;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_GOTPCREL: u32 = 9;
pub const R_X86_64_32: u32 = 10;
pub const R_X86_64_32S: u32 = 11;
pub const R_X86_64_16: u32 = 12;
pub const R_X86_64_PC16: u32 = 13;
pub const R_X86_64_8: u32 = 14;
pub const R_X86_64_PC8: u32 = 15;
pub const R_X86_64_DTPMOD64: u32 = 16;
pub const R_X86_64_DTPOFF64: u32 = 17;
pub const R_X86_64_TPOFF64: u32 = 18;
pub const R_X86_64_TLSGD: u32 = 19;
pub const R_X86_64_TLSLD: u32 = 20;
pub const R_X86_64_DTPOFF32: u32 = 21;
pub const R_X86_64_GOTTPOFF: u32 = 22;
pub const R_X86_64_TPOFF32: u32 = 23;
pub const R_X86_64_PC64: u32 = 24;
pub const R_X86_64_GOTOFF64: u32 = 25;
pub const R_X86_64_GOTPC32: u32 = 26;
pub const R_X86_64_SIZE32: u32 = 32;
pub const R_X86_64_SIZE64: u32 = 33;
pub const R_X86_64_TLSDESC_CALL: u32 = 35;
pub const R_X86_64_TLSDESC: u32 = 36;
/// An indirect function: call the resolver and store what it returns.
pub const R_X86_64_IRELATIVE: u32 = 37;
pub const R_X86_64_RELATIVE64: u32 = 38;

#[derive(Clone, Copy, Debug)]
pub struct ElfHeader {
    pub object_type: u16,
    pub machine: u16,
    pub entry: u64,
    pub program_offset: u64,
    pub program_entry_size: u16,
    pub program_count: u16,
    pub section_offset: u64,
    pub section_entry_size: u16,
    pub section_count: u16,
    pub section_name_index: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct ProgramHeader {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub virtual_address: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub align: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct SectionHeader {
    pub name_offset: u32,
    pub kind: u32,
    pub flags: u64,
    pub virtual_address: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub address_align: u64,
    pub entry_size: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicSymbol<'a> {
    pub index: u32,
    pub name: &'a str,
    pub info: u8,
    pub other: u8,
    pub section_index: u16,
    pub value: u64,
    pub size: u64,
}

impl DynamicSymbol<'_> {
    pub const fn binding(self) -> u8 {
        self.info >> 4
    }

    pub const fn symbol_type(self) -> u8 {
        self.info & 0xf
    }

    pub const fn is_defined(self) -> bool {
        self.section_index != SHN_UNDEF
    }

    /// Visibility, from the low two bits of `st_other`.
    pub const fn visibility(self) -> u8 {
        self.other & 0x3
    }

    /// True when the definition cannot be interposed from another object.
    ///
    /// Hidden and internal symbols are invisible outside their object, and a
    /// local binding was never a candidate for the global scope.
    pub const fn is_local_only(self) -> bool {
        matches!(self.visibility(), STV_HIDDEN | STV_INTERNAL) || self.binding() == STB_LOCAL
    }

    /// True when this symbol names an absolute value rather than an address.
    pub const fn is_absolute(self) -> bool {
        self.section_index == SHN_ABS
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rela {
    pub offset: u64,
    pub symbol_index: u32,
    pub kind: u32,
    pub addend: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadSpan {
    pub start: u64,
    pub end: u64,
}

impl LoadSpan {
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }

    pub fn len(self) -> Result<usize, ElfError> {
        usize::try_from(self.end - self.start).map_err(|_| ElfError::IntegerOverflow)
    }
}

#[derive(Clone, Copy)]
pub struct ElfFile<'a> {
    bytes: &'a [u8],
    header: ElfHeader,
}

impl fmt::Debug for ElfFile<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElfFile")
            .field("header", &self.header)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

impl<'a> ElfFile<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.get(0..4) != Some(b"\x7fELF") {
            return Err(ElfError::BadMagic);
        }
        if byte(bytes, 4)? != 2 {
            return Err(ElfError::NotElf64);
        }
        if byte(bytes, 5)? != 1 {
            return Err(ElfError::NotLittleEndian);
        }
        if byte(bytes, 6)? != 1 || u32_at(bytes, 20)? != 1 {
            return Err(ElfError::UnsupportedVersion);
        }
        let header = ElfHeader {
            object_type: u16_at(bytes, 16)?,
            machine: u16_at(bytes, 18)?,
            entry: u64_at(bytes, 24)?,
            program_offset: u64_at(bytes, 32)?,
            section_offset: u64_at(bytes, 40)?,
            program_entry_size: u16_at(bytes, 54)?,
            program_count: u16_at(bytes, 56)?,
            section_entry_size: u16_at(bytes, 58)?,
            section_count: u16_at(bytes, 60)?,
            section_name_index: u16_at(bytes, 62)?,
        };
        if header.machine != EM_X86_64 {
            return Err(ElfError::WrongMachine);
        }
        if !matches!(header.object_type, ET_EXEC | ET_DYN) {
            return Err(ElfError::UnsupportedObjectType);
        }
        if header.program_count != 0 && header.program_entry_size < 56 {
            return Err(ElfError::BadProgramHeaderSize);
        }
        if header.section_count != 0 && header.section_entry_size < 64 {
            return Err(ElfError::BadSectionHeaderSize);
        }
        checked_table(
            bytes,
            header.program_offset,
            header.program_entry_size,
            header.program_count,
        )?;
        checked_table(
            bytes,
            header.section_offset,
            header.section_entry_size,
            header.section_count,
        )?;
        Ok(Self { bytes, header })
    }

    pub const fn header(self) -> ElfHeader {
        self.header
    }

    /// The whole file, for the modules that index tables by file offset.
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Reads a slice of the file with bounds checking.
    pub fn slice(self, offset: usize, len: usize) -> Result<&'a [u8], ElfError> {
        let end = offset.checked_add(len).ok_or(ElfError::IntegerOverflow)?;
        self.bytes.get(offset..end).ok_or(ElfError::Truncated)
    }

    pub fn program_headers(self) -> Result<Vec<ProgramHeader>, ElfError> {
        let mut headers = Vec::with_capacity(self.header.program_count as usize);
        for index in 0..self.header.program_count {
            let entry = self.program_entry(index)?;
            let header = ProgramHeader {
                kind: u32_at(entry, 0)?,
                flags: u32_at(entry, 4)?,
                offset: u64_at(entry, 8)?,
                virtual_address: u64_at(entry, 16)?,
                file_size: u64_at(entry, 32)?,
                memory_size: u64_at(entry, 40)?,
                align: u64_at(entry, 48)?,
            };
            if header.file_size > header.memory_size {
                return Err(ElfError::FileLargerThanMemory);
            }
            checked_range(self.bytes, header.offset, header.file_size)?;
            header
                .virtual_address
                .checked_add(header.memory_size)
                .ok_or(ElfError::IntegerOverflow)?;
            headers.push(header);
        }
        Ok(headers)
    }

    pub fn section_headers(self) -> Result<Vec<SectionHeader>, ElfError> {
        let mut headers = Vec::with_capacity(self.header.section_count as usize);
        for index in 0..self.header.section_count {
            let entry = self.section_entry(index)?;
            let header = SectionHeader {
                name_offset: u32_at(entry, 0)?,
                kind: u32_at(entry, 4)?,
                flags: u64_at(entry, 8)?,
                virtual_address: u64_at(entry, 16)?,
                offset: u64_at(entry, 24)?,
                size: u64_at(entry, 32)?,
                link: u32_at(entry, 40)?,
                info: u32_at(entry, 44)?,
                address_align: u64_at(entry, 48)?,
                entry_size: u64_at(entry, 56)?,
            };
            if header.kind != 8 {
                checked_range(self.bytes, header.offset, header.size)?;
            }
            headers.push(header);
        }
        Ok(headers)
    }

    pub fn load_span(self, page_size: u64) -> Result<LoadSpan, ElfError> {
        if !page_size.is_power_of_two() {
            return Err(ElfError::InvalidAlignment);
        }
        let mut start = u64::MAX;
        let mut end = 0u64;
        for header in self.program_headers()? {
            if header.kind != PT_LOAD || header.memory_size == 0 {
                continue;
            }
            start = start.min(header.virtual_address & !(page_size - 1));
            let segment_end = header
                .virtual_address
                .checked_add(header.memory_size)
                .ok_or(ElfError::IntegerOverflow)?;
            end = end.max(align_up(segment_end, page_size)?);
        }
        if start == u64::MAX || start >= end {
            Err(ElfError::NoLoadSegments)
        } else {
            Ok(LoadSpan { start, end })
        }
    }

    pub fn tls_segment(self) -> Result<Option<ProgramHeader>, ElfError> {
        Ok(self
            .program_headers()?
            .into_iter()
            .find(|header| header.kind == PT_TLS))
    }

    pub fn dynamic_symbols(self) -> Result<Vec<DynamicSymbol<'a>>, ElfError> {
        let sections = self.section_headers()?;
        let Some(symbols) = sections.iter().find(|section| section.kind == SHT_DYNSYM) else {
            return Ok(Vec::new());
        };
        self.symbols_from_section(&sections, symbols)
    }

    /// Reads every regular and dynamic ELF symbol table.
    ///
    /// A static executable can retain a complete `.symtab` while having no
    /// dynamic symbols at all.  Code discovery and diagnostics need those local
    /// function boundaries just as much as exported ones; symbol resolution can
    /// continue to use [`Self::dynamic_symbols`] and the dynamic hash tables.
    pub fn symbols(self) -> Result<Vec<DynamicSymbol<'a>>, ElfError> {
        let sections = self.section_headers()?;
        let mut result = Vec::new();
        for symbols in sections
            .iter()
            .filter(|section| matches!(section.kind, SHT_SYMTAB | SHT_DYNSYM))
        {
            result.extend(self.symbols_from_section(&sections, symbols)?);
        }
        Ok(result)
    }

    fn symbols_from_section(
        self,
        sections: &[SectionHeader],
        symbols: &SectionHeader,
    ) -> Result<Vec<DynamicSymbol<'a>>, ElfError> {
        if symbols.entry_size < 24 || symbols.size % symbols.entry_size != 0 {
            return Err(ElfError::BadSymbolTable);
        }
        let strings = sections
            .get(symbols.link as usize)
            .ok_or(ElfError::BadSectionLink)?;
        let strings = checked_range(self.bytes, strings.offset, strings.size)?;
        let count = symbols.size / symbols.entry_size;
        let mut result = Vec::with_capacity(count as usize);
        for index in 0..count {
            let offset = symbols
                .offset
                .checked_add(index * symbols.entry_size)
                .ok_or(ElfError::IntegerOverflow)?;
            let entry = checked_range(self.bytes, offset, 24)?;
            let name = string_at(strings, u32_at(entry, 0)? as usize)?;
            result.push(DynamicSymbol {
                index: index as u32,
                name,
                info: byte(entry, 4)?,
                other: byte(entry, 5)?,
                section_index: u16_at(entry, 6)?,
                value: u64_at(entry, 8)?,
                size: u64_at(entry, 16)?,
            });
        }
        Ok(result)
    }

    pub fn relocations(self) -> Result<Vec<Rela>, ElfError> {
        let mut result = Vec::new();
        for section in self.section_headers()? {
            if section.kind != SHT_RELA {
                continue;
            }
            if section.entry_size < 24 || section.size % section.entry_size != 0 {
                return Err(ElfError::BadRelocationTable);
            }
            let count = section.size / section.entry_size;
            result.reserve(count as usize);
            for index in 0..count {
                let offset = section
                    .offset
                    .checked_add(index * section.entry_size)
                    .ok_or(ElfError::IntegerOverflow)?;
                let entry = checked_range(self.bytes, offset, 24)?;
                let info = u64_at(entry, 8)?;
                result.push(Rela {
                    offset: u64_at(entry, 0)?,
                    symbol_index: (info >> 32) as u32,
                    kind: info as u32,
                    addend: i64_at(entry, 16)?,
                });
            }
        }
        Ok(result)
    }

    pub fn needed_libraries(self) -> Result<Vec<&'a str>, ElfError> {
        self.dynamic_strings(DT_NEEDED)
    }

    pub fn soname(self) -> Result<Option<&'a str>, ElfError> {
        Ok(self.dynamic_strings(DT_SONAME)?.into_iter().next())
    }

    fn dynamic_strings(self, requested_tag: i64) -> Result<Vec<&'a str>, ElfError> {
        let sections = self.section_headers()?;
        let Some(dynamic) = sections.iter().find(|section| section.kind == SHT_DYNAMIC) else {
            return Ok(Vec::new());
        };
        if dynamic.entry_size < 16 || dynamic.size % dynamic.entry_size != 0 {
            return Err(ElfError::BadDynamicTable);
        }
        let strings = sections
            .get(dynamic.link as usize)
            .ok_or(ElfError::BadSectionLink)?;
        let strings = checked_range(self.bytes, strings.offset, strings.size)?;
        let mut result = Vec::new();
        let count = dynamic.size / dynamic.entry_size;
        for index in 0..count {
            let offset = dynamic
                .offset
                .checked_add(index * dynamic.entry_size)
                .ok_or(ElfError::IntegerOverflow)?;
            let entry = checked_range(self.bytes, offset, 16)?;
            let tag = i64_at(entry, 0)?;
            if tag == DT_NULL {
                break;
            }
            if tag == requested_tag {
                result.push(string_at(strings, u64_at(entry, 8)? as usize)?);
            }
        }
        Ok(result)
    }

    fn program_entry(self, index: u16) -> Result<&'a [u8], ElfError> {
        let offset = self
            .header
            .program_offset
            .checked_add(index as u64 * self.header.program_entry_size as u64)
            .ok_or(ElfError::IntegerOverflow)?;
        checked_range(self.bytes, offset, self.header.program_entry_size as u64)
    }

    fn section_entry(self, index: u16) -> Result<&'a [u8], ElfError> {
        let offset = self
            .header
            .section_offset
            .checked_add(index as u64 * self.header.section_entry_size as u64)
            .ok_or(ElfError::IntegerOverflow)?;
        checked_range(self.bytes, offset, self.header.section_entry_size as u64)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    NotElf64,
    NotLittleEndian,
    UnsupportedVersion,
    WrongMachine,
    UnsupportedObjectType,
    BadProgramHeaderSize,
    BadSectionHeaderSize,
    FileLargerThanMemory,
    IntegerOverflow,
    NoLoadSegments,
    InvalidAlignment,
    BadSectionLink,
    BadSymbolTable,
    BadRelocationTable,
    BadDynamicTable,
    InvalidString,
    InvalidUtf8,
    /// A dynamic table entry pointed outside every `PT_LOAD` segment.
    UnmappedDynamicAddress,
    /// `DT_SYMTAB` without `DT_STRTAB`: the symbols could not be named.
    MissingStringTable,
    /// The 32-bit `DT_REL` form, which x86_64 objects never use.
    UnsupportedRelFormat,
    /// A hash table's header or bucket array was malformed.
    BadHashTable,
    /// A version table's header or entry chain was malformed.
    BadVersionTable,
}

fn checked_table(bytes: &[u8], offset: u64, entry_size: u16, count: u16) -> Result<(), ElfError> {
    let size = (entry_size as u64)
        .checked_mul(count as u64)
        .ok_or(ElfError::IntegerOverflow)?;
    checked_range(bytes, offset, size).map(|_| ())
}

fn checked_range(bytes: &[u8], offset: u64, size: u64) -> Result<&[u8], ElfError> {
    let start = usize::try_from(offset).map_err(|_| ElfError::IntegerOverflow)?;
    let size = usize::try_from(size).map_err(|_| ElfError::IntegerOverflow)?;
    let end = start.checked_add(size).ok_or(ElfError::IntegerOverflow)?;
    bytes.get(start..end).ok_or(ElfError::Truncated)
}

pub(crate) fn string_at(bytes: &[u8], offset: usize) -> Result<&str, ElfError> {
    let suffix = bytes.get(offset..).ok_or(ElfError::InvalidString)?;
    let length = suffix
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(ElfError::InvalidString)?;
    core::str::from_utf8(&suffix[..length]).map_err(|_| ElfError::InvalidUtf8)
}

fn align_up(value: u64, align: u64) -> Result<u64, ElfError> {
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or(ElfError::IntegerOverflow)
}

pub(crate) fn byte(bytes: &[u8], offset: usize) -> Result<u8, ElfError> {
    bytes.get(offset).copied().ok_or(ElfError::Truncated)
}

pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    let bytes = bytes.get(offset..offset + 2).ok_or(ElfError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    let bytes = bytes.get(offset..offset + 4).ok_or(ElfError::Truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    let bytes = bytes.get(offset..offset + 8).ok_or(ElfError::Truncated)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

pub(crate) fn i64_at(bytes: &[u8], offset: usize) -> Result<i64, ElfError> {
    Ok(u64_at(bytes, offset)? as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_elf() -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        bytes[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes
    }

    #[test]
    fn validates_a_minimal_elf64_header() {
        let bytes = minimal_elf();
        let parsed = ElfFile::parse(&bytes).unwrap();
        assert_eq!(parsed.header().object_type, ET_DYN);
        assert!(parsed.program_headers().unwrap().is_empty());
    }

    #[test]
    fn rejects_truncated_and_wrong_architecture_files() {
        assert!(matches!(ElfFile::parse(b"ELF"), Err(ElfError::BadMagic)));
        let mut bytes = minimal_elf();
        bytes[18..20].copy_from_slice(&3u16.to_le_bytes());
        assert!(matches!(
            ElfFile::parse(&bytes),
            Err(ElfError::WrongMachine)
        ));
    }

    #[test]
    fn regular_symbol_tables_expose_local_function_roots() {
        let mut bytes = vec![0u8; 320];
        bytes[..64].copy_from_slice(&minimal_elf());
        bytes[40..48].copy_from_slice(&64u64.to_le_bytes());
        bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&3u16.to_le_bytes());

        // Section 1: string table at 256.
        bytes[128 + 4..128 + 8].copy_from_slice(&3u32.to_le_bytes()); // SHT_STRTAB
        bytes[128 + 24..128 + 32].copy_from_slice(&256u64.to_le_bytes());
        bytes[128 + 32..128 + 40].copy_from_slice(&5u64.to_le_bytes());
        bytes[256..261].copy_from_slice(b"\0foo\0");

        // Section 2: two-entry regular symbol table at 272, linked to section 1.
        bytes[192 + 4..192 + 8].copy_from_slice(&SHT_SYMTAB.to_le_bytes());
        bytes[192 + 24..192 + 32].copy_from_slice(&272u64.to_le_bytes());
        bytes[192 + 32..192 + 40].copy_from_slice(&48u64.to_le_bytes());
        bytes[192 + 40..192 + 44].copy_from_slice(&1u32.to_le_bytes());
        bytes[192 + 56..192 + 64].copy_from_slice(&24u64.to_le_bytes());
        let symbol = 272 + 24;
        bytes[symbol..symbol + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[symbol + 4] = STT_FUNC;
        bytes[symbol + 6..symbol + 8].copy_from_slice(&1u16.to_le_bytes());
        bytes[symbol + 8..symbol + 16].copy_from_slice(&0x1234u64.to_le_bytes());
        bytes[symbol + 16..symbol + 24].copy_from_slice(&0x20u64.to_le_bytes());

        let elf = ElfFile::parse(&bytes).unwrap();
        assert!(elf.dynamic_symbols().unwrap().is_empty());
        let symbols = elf.symbols().unwrap();
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[1].name, "foo");
        assert_eq!(symbols[1].symbol_type(), STT_FUNC);
        assert_eq!(symbols[1].value, 0x1234);
    }
}
