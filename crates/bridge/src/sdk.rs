use crate::{ExportKind, Module, Result, invalid};
use std::collections::HashMap;

const PAGE: usize = 4096;
const MAX_IMAGE: usize = 32 * 1024 * 1024;
const PHNUM: usize = 5;
const SHNUM: usize = 11;
const THUNK_SIZE: usize = 16;

#[derive(Debug, Clone)]
pub struct ImportLibraryInfo {
    pub module_id: u32,
    pub soname: String,
    pub file_size: usize,
    pub image_size: usize,
    pub exports: Vec<String>,
}

struct Layout {
    bytes: Vec<u8>,
}

fn align(value: usize, alignment: usize) -> usize {
    // Callers use bounded export counts and fixed power-of-two alignments.
    (value + alignment - 1) & !(alignment - 1)
}

fn u16_at(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn u32_at(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn u64_at(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let source = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| invalid("truncated ELF u16"))?;
    Ok(u16::from_le_bytes(source.try_into().unwrap()))
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let source = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| invalid("truncated ELF u32"))?;
    Ok(u32::from_le_bytes(source.try_into().unwrap()))
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let source = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| invalid("truncated ELF u64"))?;
    Ok(u64::from_le_bytes(source.try_into().unwrap()))
}

fn add_string(table: &mut Vec<u8>, value: &str) -> usize {
    let offset = table.len();
    table.extend_from_slice(value.as_bytes());
    table.push(0);
    offset
}

fn elf_hash(name: &str) -> u32 {
    let mut hash = 0u32;
    for byte in name.bytes() {
        hash = (hash << 4).wrapping_add(byte as u32);
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    hash
}

fn layout(module: &Module) -> Result<Layout> {
    module.validate()?;
    let count = module.exports.len();
    let mut dynstr = vec![0];
    let soname_name = add_string(&mut dynstr, &module.soname);
    let export_names: Vec<_> = module
        .exports
        .iter()
        .map(|export| add_string(&mut dynstr, &export.name))
        .collect();
    // Keep every supported version explicit in ELF metadata. A single export
    // implementation may intentionally supply several reviewed ABI versions.
    let mut version_names = vec![(module.soname.as_str(), soname_name)];
    let mut version_indices = HashMap::new();
    for export in &module.exports {
        for version in &export.versions {
            if !version_indices.contains_key(version.as_str()) {
                let index = version_names.len() + 1;
                if index >= 0x8000 {
                    return Err(invalid("too many distinct ABI versions"));
                }
                version_indices.insert(version.as_str(), index as u16);
                version_names.push((version.as_str(), add_string(&mut dynstr, version)));
            }
        }
    }
    let mut symbol_rows = Vec::new();
    for (index, export) in module.exports.iter().enumerate() {
        if export.versions.is_empty() {
            symbol_rows.push((index, 1u16));
        } else {
            for version in &export.versions {
                let hidden = if export.default_version.as_ref() == Some(version) {
                    0
                } else {
                    0x8000
                };
                symbol_rows.push((index, version_indices[version.as_str()] | hidden));
            }
        }
    }
    let symbol_count = symbol_rows.len();
    let mut shstr = vec![0];
    let section_names: Vec<_> = [
        ".text",
        ".kinakaze.bind",
        ".dynstr",
        ".dynsym",
        ".hash",
        ".dynamic",
        ".shstrtab",
        ".gnu.version",
        ".gnu.version_d",
        ".data",
    ]
    .into_iter()
    .map(|name| add_string(&mut shstr, name))
    .collect();
    let dynstr_offset = 64 + PHNUM * 56;
    let dynsym_offset = align(dynstr_offset + dynstr.len(), 8);
    let dynsym_size = (symbol_count + 1) * 24;
    let hash_offset = align(dynsym_offset + dynsym_size, 8);
    // A valid SysV hash table with one bucket chaining every exported symbol.
    let hash_size = (2 + 1 + symbol_count + 1) * 4;
    let versym_offset = align(hash_offset + hash_size, 2);
    let versym_size = (symbol_count + 1) * 2;
    let verdef_offset = align(versym_offset + versym_size, 4);
    let verdef_size = version_names.len() * 28;
    let dynamic_offset = align(verdef_offset + verdef_size, 8);
    let dynamic_size = 10 * 16;
    let shstr_offset = align(dynamic_offset + dynamic_size, 4);
    let shoff = align(shstr_offset + shstr.len(), 8);
    let text = align(shoff + SHNUM * 64, PAGE);
    let text_size = align(count * THUNK_SIZE, PAGE);
    let slots = text + text_size;
    let data_alignment = module
        .exports
        .iter()
        .filter(|export| export.kind == ExportKind::Object)
        .map(|export| export.alignment as usize)
        .max()
        .unwrap_or(1);
    let data = align(slots + count * 8, data_alignment);
    let mut object_offsets = vec![0usize; count];
    let mut data_end = data;
    for (index, export) in module.exports.iter().enumerate() {
        if export.kind == ExportKind::Object {
            data_end = align(data_end, export.alignment as usize);
            object_offsets[index] = data_end;
            data_end = data_end
                .checked_add(export.size as usize)
                .filter(|end| *end <= MAX_IMAGE)
                .ok_or_else(|| invalid("generated object storage exceeds size limit"))?;
        }
    }
    let size = align(data_end, PAGE);
    if size > MAX_IMAGE {
        return Err(invalid("generated import library exceeds size limit"));
    }
    let mut bytes = vec![0u8; size];

    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    u16_at(&mut bytes, 16, 3); // ET_DYN
    u16_at(&mut bytes, 18, 62); // EM_X86_64
    u32_at(&mut bytes, 20, 1);
    u64_at(&mut bytes, 32, 64); // e_phoff
    u64_at(&mut bytes, 40, shoff as u64);
    u16_at(&mut bytes, 52, 64);
    u16_at(&mut bytes, 54, 56);
    u16_at(&mut bytes, 56, PHNUM as u16);
    u16_at(&mut bytes, 58, 64);
    u16_at(&mut bytes, 60, SHNUM as u16);
    u16_at(&mut bytes, 62, 7);

    let phdrs = [
        (1u32, 4u32, 0, text, PAGE),             // R headers / metadata
        (1, 5, text, text_size, PAGE),           // RX SysV thunks
        (1, 6, slots, size - slots, PAGE),       // RW binding slots
        (2, 4, dynamic_offset, dynamic_size, 8), // PT_DYNAMIC
        (0x6474_e551, 6, 0, 0, 16),              // PT_GNU_STACK, non-executable
    ];
    for (index, (kind, flags, offset, length, alignment)) in phdrs.into_iter().enumerate() {
        let at = 64 + index * 56;
        u32_at(&mut bytes, at, kind);
        u32_at(&mut bytes, at + 4, flags);
        u64_at(&mut bytes, at + 8, offset as u64);
        u64_at(&mut bytes, at + 16, offset as u64);
        u64_at(&mut bytes, at + 24, offset as u64);
        u64_at(&mut bytes, at + 32, length as u64);
        u64_at(&mut bytes, at + 40, length as u64);
        u64_at(&mut bytes, at + 48, alignment as u64);
    }
    bytes[dynstr_offset..dynstr_offset + dynstr.len()].copy_from_slice(&dynstr);
    for (row, &(index, version)) in symbol_rows.iter().enumerate() {
        let at = dynsym_offset + (row + 1) * 24;
        u32_at(&mut bytes, at, export_names[index] as u32);
        match module.exports[index].kind {
            ExportKind::Function => {
                bytes[at + 4] = 0x12; // STB_GLOBAL | STT_FUNC
                u16_at(&mut bytes, at + 6, 1); // .text
                u64_at(&mut bytes, at + 8, (text + index * THUNK_SIZE) as u64);
                u64_at(&mut bytes, at + 16, 6);
            }
            ExportKind::Object => {
                bytes[at + 4] = 0x11; // STB_GLOBAL | STT_OBJECT
                // These files are compiler inputs only. A section-backed data
                // definition lets GNU ld resolve versioned DSO dependencies and
                // emit ordinary data relocations. At runtime the SONAME selects
                // the native image, whose storage the loader binds directly.
                u16_at(&mut bytes, at + 6, 10); // .data
                u64_at(&mut bytes, at + 8, object_offsets[index] as u64);
                u64_at(&mut bytes, at + 16, module.exports[index].size);
            }
        }
        u16_at(&mut bytes, versym_offset + (row + 1) * 2, version);
    }
    u32_at(&mut bytes, hash_offset, 1); // nbucket
    u32_at(&mut bytes, hash_offset + 4, (symbol_count + 1) as u32); // nchain
    u32_at(&mut bytes, hash_offset + 8, 1); // bucket[0]
    for index in 1..symbol_count {
        u32_at(&mut bytes, hash_offset + 12 + index * 4, (index + 1) as u32);
    }
    for (index, (name, string_offset)) in version_names.iter().enumerate() {
        let at = verdef_offset + index * 28;
        u16_at(&mut bytes, at, 1); // VER_DEF_CURRENT
        u16_at(&mut bytes, at + 2, u16::from(index == 0)); // VER_FLG_BASE
        u16_at(&mut bytes, at + 4, (index + 1) as u16);
        u16_at(&mut bytes, at + 6, 1); // vd_cnt
        u32_at(&mut bytes, at + 8, elf_hash(name));
        u32_at(&mut bytes, at + 12, 20); // vd_aux
        u32_at(
            &mut bytes,
            at + 16,
            if index + 1 == version_names.len() {
                0
            } else {
                28
            },
        );
        u32_at(&mut bytes, at + 20, *string_offset as u32);
    }
    let dynamics = [
        (4u64, hash_offset),
        (5, dynstr_offset),
        (6, dynsym_offset),
        (10, dynstr.len()),
        (11, 24),
        (14, soname_name),
        (0x6fff_fff0, versym_offset),
        (0x6fff_fffc, verdef_offset),
        (0x6fff_fffd, version_names.len()),
        (0, 0),
    ];
    for (index, (tag, value)) in dynamics.into_iter().enumerate() {
        u64_at(&mut bytes, dynamic_offset + index * 16, tag);
        u64_at(&mut bytes, dynamic_offset + index * 16 + 8, value as u64);
    }
    bytes[shstr_offset..shstr_offset + shstr.len()].copy_from_slice(&shstr);

    // name, type, flags, offset, size, link, info, alignment, entry size
    let sections = [
        (1u32, 6u64, text, count * THUNK_SIZE, 0u32, 0u32, 16, 0),
        (1, 3, slots, count * 8, 0, 0, 8, 8),
        (3, 2, dynstr_offset, dynstr.len(), 0, 0, 1, 0),
        (11, 2, dynsym_offset, dynsym_size, 3, 1, 8, 24),
        (5, 2, hash_offset, hash_size, 4, 0, 4, 4),
        (6, 2, dynamic_offset, dynamic_size, 3, 0, 8, 16),
        (3, 0, shstr_offset, shstr.len(), 0, 0, 1, 0),
        (0x6fff_ffff, 2, versym_offset, versym_size, 4, 0, 2, 2),
        (
            0x6fff_fffd,
            2,
            verdef_offset,
            verdef_size,
            3,
            version_names.len() as u32,
            4,
            0,
        ),
        (1, 3, data, data_end - data, 0, 0, data_alignment as u64, 0),
    ];
    for (index, (kind, flags, offset, length, link, info, alignment, entsize)) in
        sections.into_iter().enumerate()
    {
        let at = shoff + (index + 1) * 64;
        u32_at(&mut bytes, at, section_names[index] as u32);
        u32_at(&mut bytes, at + 4, kind);
        u64_at(&mut bytes, at + 8, flags);
        u64_at(
            &mut bytes,
            at + 16,
            if flags & 2 != 0 { offset as u64 } else { 0 },
        );
        u64_at(&mut bytes, at + 24, offset as u64);
        u64_at(&mut bytes, at + 32, length as u64);
        u32_at(&mut bytes, at + 40, link);
        u32_at(&mut bytes, at + 44, info);
        u64_at(&mut bytes, at + 48, alignment);
        u64_at(&mut bytes, at + 56, entsize);
    }
    for index in 0..count {
        let at = text + index * THUNK_SIZE;
        bytes[at..at + THUNK_SIZE].fill(0xcc);
        bytes[at] = 0xff;
        bytes[at + 1] = 0x25; // jmp qword ptr [rip + disp32]
        let displacement = i32::try_from(slots + index * 8 - (at + 6))
            .map_err(|_| invalid("binding slot is outside RIP-relative range"))?;
        bytes[at + 2..at + 6].copy_from_slice(&displacement.to_le_bytes());
    }
    Ok(Layout { bytes })
}

/// Generate a deterministic ELF64 ET_DYN, never a renamed PE image.
pub fn generate_import_library(module: &Module) -> Result<Vec<u8>> {
    Ok(layout(module)?.bytes)
}

fn checked_range(offset: u64, length: u64, total: usize) -> Result<std::ops::Range<usize>> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid("ELF range overflow"))?;
    if end > total as u64 {
        return Err(invalid("ELF range outside image"));
    }
    Ok(offset as usize..end as usize)
}

fn validate_shape(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 64
        || bytes.len() > MAX_IMAGE
        || &bytes[..7] != b"\x7fELF\x02\x01\x01"
        || read_u16(bytes, 16)? != 3
        || read_u16(bytes, 18)? != 62
        || read_u32(bytes, 20)? != 1
        || read_u16(bytes, 52)? != 64
        || read_u16(bytes, 54)? != 56
        || read_u16(bytes, 58)? != 64
    {
        return Err(invalid(
            "expected generated ELF64 little-endian x86-64 ET_DYN",
        ));
    }
    let phnum = read_u16(bytes, 56)? as usize;
    let shnum = read_u16(bytes, 60)? as usize;
    if phnum != PHNUM || shnum != SHNUM || read_u16(bytes, 62)? as usize >= shnum {
        return Err(invalid("unexpected ELF header table counts"));
    }
    let phdrs = checked_range(read_u64(bytes, 32)?, (phnum * 56) as u64, bytes.len())?;
    let shdrs = checked_range(read_u64(bytes, 40)?, (shnum * 64) as u64, bytes.len())?;
    let mut loads: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> = Vec::new();
    for index in 0..phnum {
        let at = phdrs.start + index * 56;
        let kind = read_u32(bytes, at)?;
        let flags = read_u32(bytes, at + 4)?;
        let offset = read_u64(bytes, at + 8)?;
        let vaddr = read_u64(bytes, at + 16)?;
        let filesz = read_u64(bytes, at + 32)?;
        let memsz = read_u64(bytes, at + 40)?;
        let alignment = read_u64(bytes, at + 48)?;
        let file = checked_range(offset, filesz, bytes.len())?;
        if kind == 1 {
            if flags & 3 == 3
                || flags & !7 != 0
                || filesz > memsz
                || memsz == 0
                || alignment != PAGE as u64
                || offset % alignment != 0
                || vaddr % alignment != 0
            {
                return Err(invalid("invalid ELF load segment or W^X violation"));
            }
            let memory = checked_range(vaddr, memsz, MAX_IMAGE)?;
            if loads.iter().any(|(f, m)| {
                (file.start < f.end && f.start < file.end)
                    || (memory.start < m.end && m.start < memory.end)
            }) {
                return Err(invalid("overlapping ELF load segments"));
            }
            loads.push((file, memory));
        }
    }
    if loads.len() != 3 {
        return Err(invalid("expected three ELF load segments"));
    }
    for index in 0..shnum {
        let at = shdrs.start + index * 64;
        let kind = read_u32(bytes, at + 4)?;
        let length = read_u64(bytes, at + 32)?;
        if kind == 4 || kind == 9 || kind == 19 {
            // RELA, REL, RELR
            return Err(invalid(
                "generated import libraries do not permit ELF relocations",
            ));
        }
        checked_range(read_u64(bytes, at + 24)?, length, bytes.len())?;
        let entsize = read_u64(bytes, at + 56)?;
        if entsize != 0 && length % entsize != 0 {
            return Err(invalid("ELF section size is not a multiple of entry size"));
        }
    }
    Ok(())
}

fn validated_layout(bytes: &[u8], module: &Module) -> Result<Layout> {
    validate_shape(bytes)?;
    let expected = layout(module)?;
    // Exact equality also verifies every dynamic tag/string/symbol, metadata
    // descriptor, thunk instruction, padding byte and initially empty slot.
    // No unchecked file-controlled offset is ever used for native mapping.
    if bytes != expected.bytes {
        return Err(invalid(
            "ELF is not the canonical import library for this native export table",
        ));
    }
    Ok(expected)
}

/// Validate without loading a DLL or allocating executable memory.
pub fn inspect_import_library(bytes: &[u8], module: &Module) -> Result<ImportLibraryInfo> {
    validated_layout(bytes, module)?;
    Ok(ImportLibraryInfo {
        module_id: module.id,
        soname: module.soname.clone(),
        file_size: bytes.len(),
        image_size: bytes.len(),
        exports: module
            .exports
            .iter()
            .map(|export| export.name.clone())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Export;

    fn modules() -> Vec<Module> {
        let mut modules = crate::module::test_modules().modules;
        // Exercise the original simple layout independently of the evolving
        // full ABI catalog used by packaged distributions.
        for module in &mut modules {
            module.exports = vec![
                Export::function("one", "native_one"),
                Export::function("two", "native_two"),
            ];
        }
        modules.truncate(3);
        modules
    }

    #[test]
    fn object_symbols_have_no_shadow_storage_and_versions_are_gnu_tables() {
        let mut module = modules().remove(1);
        module.exports[0].versions = vec!["GLIBC_2.2.5".into(), "GLIBC_2.34".into()];
        module.exports[0].default_version = Some("GLIBC_2.34".into());
        module.exports[1].kind = ExportKind::Object;
        module.exports[1].size = 16;
        module.exports[1].alignment = 8;
        let bytes = generate_import_library(&module).unwrap();
        inspect_import_library(&bytes, &module).unwrap();
        let shoff = read_u64(&bytes, 40).unwrap() as usize;
        let dynsym = read_u64(&bytes, shoff + 4 * 64 + 24).unwrap() as usize;
        let versym = read_u64(&bytes, shoff + 8 * 64 + 24).unwrap() as usize;
        let verdef = read_u64(&bytes, shoff + 9 * 64 + 24).unwrap() as usize;
        assert_eq!(read_u16(&bytes, versym + 2).unwrap(), 0x8002);
        assert_eq!(read_u16(&bytes, versym + 4).unwrap(), 3);
        assert_eq!(read_u16(&bytes, versym + 6).unwrap(), 1);
        assert_eq!(read_u16(&bytes, verdef + 2).unwrap(), 1); // base
        assert_eq!(
            read_u32(&bytes, verdef + 28 + 8).unwrap(),
            elf_hash("GLIBC_2.2.5")
        );
        let object = dynsym + 3 * 24;
        assert_eq!(bytes[object + 4], 0x11);
        assert_eq!(read_u16(&bytes, object + 6).unwrap(), 10);
        let data = read_u64(&bytes, shoff + 10 * 64 + 16).unwrap();
        assert_ne!(data, 0);
        assert_eq!(read_u64(&bytes, object + 8).unwrap(), data);
        assert_eq!(data % module.exports[1].alignment, 0);
        assert_eq!(read_u64(&bytes, object + 16).unwrap(), 16);
    }

    #[test]
    fn large_export_table_is_bounded_and_generates_canonical_elf() {
        let mut module = modules().remove(1);
        module.exports = (0..16_384)
            .map(|index| Export::function(format!("symbol_{index}"), "same_native_alias"))
            .collect();
        let bytes = generate_import_library(&module).unwrap();
        assert_eq!(
            inspect_import_library(&bytes, &module)
                .unwrap()
                .exports
                .len(),
            16_384
        );
        module.exports.push(Export::function("too_many", "native"));
        assert!(generate_import_library(&module).is_err());
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn generated_modules_are_real_elf_with_soname_and_symbols() {
        for module in modules() {
            let bytes = generate_import_library(&module).unwrap();
            let info = inspect_import_library(&bytes, &module).unwrap();
            assert_eq!(&bytes[..4], b"\x7fELF");
            assert_eq!(read_u16(&bytes, 16).unwrap(), 3);
            assert_eq!(read_u16(&bytes, 18).unwrap(), 62);
            assert_eq!(info.soname, module.soname);
            assert_eq!(info.exports.len(), module.exports.len());
            let dynamic = read_u64(&bytes, 64 + 3 * 56 + 8).unwrap() as usize;
            let dynstr = read_u64(&bytes, dynamic + 16 + 8).unwrap() as usize;
            let soname_index = read_u64(&bytes, dynamic + 5 * 16 + 8).unwrap() as usize;
            let at = dynstr + soname_index;
            assert_eq!(
                &bytes[at..at + module.soname.len()],
                module.soname.as_bytes()
            );
            assert_eq!(bytes[at + module.soname.len()], 0);
            let dynsym = read_u64(&bytes, dynamic + 2 * 16 + 8).unwrap() as usize;
            for (index, export) in module.exports.iter().enumerate() {
                let at = dynsym + (index + 1) * 24;
                assert_eq!(bytes[at + 4], 0x12);
                let name = dynstr + read_u32(&bytes, at).unwrap() as usize;
                assert_eq!(
                    &bytes[name..name + export.name.len()],
                    export.name.as_bytes()
                );
                let thunk = read_u64(&bytes, at + 8).unwrap() as usize;
                assert_eq!(&bytes[thunk..thunk + 2], &[0xff, 0x25]);
            }
        }
    }

    #[test]
    fn truncated_and_overflowing_images_are_rejected_without_panics() {
        let module = modules().remove(1);
        let bytes = generate_import_library(&module).unwrap();
        for length in (0..64).chain([64, 399, 400, bytes.len() - 1]) {
            assert!(inspect_import_library(&bytes[..length], &module).is_err());
        }
        for offset in [32, 40, 64 + 8, 64 + 16, 64 + 32, 64 + 40] {
            let mut broken = bytes.clone();
            u64_at(&mut broken, offset, u64::MAX);
            assert!(inspect_import_library(&broken, &module).is_err());
        }
    }

    #[test]
    fn overlaps_wx_relocations_code_and_metadata_changes_are_rejected() {
        let module = modules().remove(1);
        let expected = layout(&module).unwrap();
        let bytes = expected.bytes;
        let mut broken = bytes.clone();
        u64_at(&mut broken, 64 + 56 + 16, 0);
        assert!(inspect_import_library(&broken, &module).is_err());
        let mut broken = bytes.clone();
        u32_at(&mut broken, 64 + 56 + 4, 7);
        assert!(inspect_import_library(&broken, &module).is_err());
        let mut broken = bytes.clone();
        let shoff = read_u64(&broken, 40).unwrap() as usize;
        u32_at(&mut broken, shoff + 64 + 4, 4);
        assert!(inspect_import_library(&broken, &module).is_err());
        for offset in [
            read_u64(&bytes, 64 + 56 + 8).unwrap() as usize,
            read_u64(&bytes, 64 + 112 + 8).unwrap() as usize,
            bytes.len() - 1,
        ] {
            let mut broken = bytes.clone();
            broken[offset] ^= 1;
            assert!(inspect_import_library(&broken, &module).is_err());
        }
        let mut different = module.clone();
        different.exports[0].pe_export.push_str("_other");
        assert!(inspect_import_library(&bytes, &different).is_ok());
        different.exports[0].name.push_str("_other");
        assert!(inspect_import_library(&bytes, &different).is_err());
    }
}
