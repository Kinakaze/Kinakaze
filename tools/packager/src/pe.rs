//! Read-only PE checks: packaging never executes provider initialization code.
use kinakaze_v2_bridge::{Export, ExportKind, Module, ModuleLifecycle};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const EXECUTE: u32 = 0x2000_0000;
const READ: u32 = 0x4000_0000;
const MAX_FORWARD_DEPTH: usize = 256;
const MANAGEMENT_EXPORTS: [&str; 4] = [
    "kinakaze_provider_initialize_v1",
    "kinakaze_provider_state_define_v1",
    "kinakaze_provider_state_read_v1",
    "kinakaze_provider_state_write_v1",
];

pub fn imports(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(import_entries(bytes)?
        .into_iter()
        .map(|(_, name)| name)
        .collect())
}

fn import_entries(bytes: &[u8]) -> Result<Vec<(usize, String)>> {
    let pe = u32_at(bytes, 0x3c)? as usize;
    if range(bytes, 0, 2)? != b"MZ" || range(bytes, pe, 4)? != b"PE\0\0" {
        return Err("invalid native PE image".into());
    }
    let optional = pe + 24;
    if u16_at(bytes, optional)? != 0x20b {
        return Err("native imports require PE32+".into());
    }
    let sections_at = optional + u16_at(bytes, pe + 20)? as usize;
    let mut sections = Vec::new();
    for index in 0..u16_at(bytes, pe + 6)? as usize {
        let at = sections_at + index * 40;
        sections.push(Section {
            virtual_size: u32_at(bytes, at + 8)?,
            rva: u32_at(bytes, at + 12)?,
            file_size: u32_at(bytes, at + 16)?,
            file: u32_at(bytes, at + 20)?,
            flags: u32_at(bytes, at + 36)?,
        });
    }
    let mut result = Vec::new();
    // IMAGE_IMPORT_DESCRIPTOR and ImgDelayDescr. Delay descriptors use RVAs.
    for (directory, stride, name_field) in [(1, 20, 12), (13, 32, 4)] {
        if u32_at(bytes, optional + 108)? <= directory {
            continue;
        }
        let rva = u32_at(bytes, optional + 112 + directory as usize * 8)?;
        let size = u32_at(bytes, optional + 116 + directory as usize * 8)? as usize;
        if rva == 0 {
            continue;
        }
        let start = from_rva(bytes, &sections, rva, size)?;
        let mut terminated = false;
        for relative in (0..size).step_by(stride) {
            let at = start + relative;
            let descriptor = range(bytes, at, stride)?;
            if descriptor.iter().all(|&byte| byte == 0) {
                terminated = true;
                break;
            }
            if directory == 13 && u32_at(bytes, at)? != 1 {
                return Err("delay imports require RVA addressing".into());
            }
            let offset = from_rva(bytes, &sections, u32_at(bytes, at + name_field)?, 1)?;
            let end = bytes[offset..]
                .iter()
                .take(256)
                .position(|&b| b == 0)
                .ok_or("unterminated native import name")?;
            let name = std::str::from_utf8(range(bytes, offset, end)?)?;
            module_key(name)?;
            result.push((offset, name.to_owned()));
        }
        if !terminated {
            return Err("unterminated native import descriptors".into());
        }
    }
    Ok(result)
}

fn range(bytes: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    let end = offset.checked_add(size).ok_or("PE offset overflow")?;
    bytes
        .get(offset..end)
        .ok_or_else(|| "PE range outside file".into())
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(range(bytes, offset, 2)?.try_into()?))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(range(bytes, offset, 4)?.try_into()?))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(range(bytes, offset, 8)?.try_into()?))
}

struct Section {
    rva: u32,
    virtual_size: u32,
    file: u32,
    file_size: u32,
    flags: u32,
}
impl Section {
    fn virtual_end(&self) -> u64 {
        self.rva as u64 + self.virtual_size.max(self.file_size) as u64
    }
}

fn from_rva(bytes: &[u8], sections: &[Section], rva: u32, size: usize) -> Result<usize> {
    for section in sections {
        if let Some(relative) = rva.checked_sub(section.rva)
            && (relative as u64)
                .checked_add(size as u64)
                .is_some_and(|end| end <= section.file_size as u64)
        {
            let offset = (section.file as usize)
                .checked_add(relative as usize)
                .ok_or("PE offset overflow")?;
            range(bytes, offset, size)?;
            return Ok(offset);
        }
    }
    Err("PE RVA is not backed by section data".into())
}

struct Pe<'a> {
    bytes: &'a [u8],
    sections: Vec<Section>,
    image_base: u64,
    export_rva: u32,
    export_end: u32,
    ordinal_base: u32,
    addresses: Vec<u32>,
    names: HashMap<&'a str, usize>,
}
enum ForwardSymbol<'a> {
    Name(&'a str),
    Ordinal(u32),
}
struct Forwarder<'a> {
    module: &'a str,
    symbol: ForwardSymbol<'a>,
}

fn module_key(name: &str) -> Result<String> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || name.ends_with('.')
        || name.contains("..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("invalid forwarded DLL module name".into());
    }
    let mut key = name.to_ascii_lowercase();
    if key.ends_with(".dll") {
        key.truncate(key.len() - 4);
    }
    if key.is_empty() {
        return Err("empty forwarded DLL module name".into());
    }
    Ok(key)
}

fn parse_forwarder(value: &str) -> Result<Forwarder<'_>> {
    let (module, symbol) = value
        .rsplit_once('.')
        .ok_or("PE forwarder must be module.symbol")?;
    module_key(module)?;
    let symbol = if let Some(ordinal) = symbol.strip_prefix('#') {
        if ordinal.is_empty() || !ordinal.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid forwarded PE ordinal".into());
        }
        let ordinal = ordinal
            .parse::<u32>()
            .map_err(|_| "forwarded PE ordinal overflow")?;
        if ordinal == 0 || ordinal > u16::MAX as u32 {
            return Err("forwarded PE ordinal must be 1..=65535".into());
        }
        ForwardSymbol::Ordinal(ordinal)
    } else {
        if symbol.is_empty()
            || symbol.len() > 4096
            || !symbol
                .bytes()
                .all(|b| b.is_ascii_graphic() && !matches!(b, b'.' | b'/' | b'\\' | b':' | b'#'))
        {
            return Err("invalid forwarded PE symbol name".into());
        }
        ForwardSymbol::Name(symbol)
    };
    Ok(Forwarder { module, symbol })
}

impl<'a> Pe<'a> {
    fn parse(bytes: &'a [u8], name: &str) -> Result<Self> {
        if range(bytes, 0, 2)? != b"MZ" {
            return Err(format!("{name} is not a PE DLL").into());
        }
        let pe = u32_at(bytes, 0x3c)? as usize;
        range(bytes, pe, 24)?;
        if range(bytes, pe, 4)? != b"PE\0\0"
            || u16_at(bytes, pe + 4)? != 0x8664
            || u16_at(bytes, pe + 22)? & 0x2000 == 0
        {
            return Err(format!("{name} must be an x86-64 PE DLL").into());
        }
        let section_count = u16_at(bytes, pe + 6)? as usize;
        let optional_size = u16_at(bytes, pe + 20)? as usize;
        let optional = pe.checked_add(24).ok_or("PE header overflow")?;
        range(bytes, optional, optional_size)?;
        if section_count == 0
            || section_count > 96
            || optional_size < 120
            || u16_at(bytes, optional)? != 0x20b
            || u32_at(bytes, optional + 108)? == 0
        {
            return Err("invalid PE32+ optional header".into());
        }
        let image_base = u64_at(bytes, optional + 24)?;
        let export_rva = u32_at(bytes, optional + 112)?;
        let export_size = u32_at(bytes, optional + 116)?;
        let export_end = export_rva
            .checked_add(export_size)
            .ok_or("PE export range overflow")?;
        if export_rva == 0 || export_size < 40 {
            return Err("PE DLL has no export directory".into());
        }
        let section_offset = optional
            .checked_add(optional_size)
            .ok_or("PE header overflow")?;
        range(bytes, section_offset, section_count * 40)?;
        let mut sections: Vec<Section> = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let at = section_offset + index * 40;
            let section = Section {
                virtual_size: u32_at(bytes, at + 8)?,
                rva: u32_at(bytes, at + 12)?,
                file_size: u32_at(bytes, at + 16)?,
                file: u32_at(bytes, at + 20)?,
                flags: u32_at(bytes, at + 36)?,
            };
            range(bytes, section.file as usize, section.file_size as usize)?;
            if section.virtual_end() > u32::MAX as u64 + 1
                || sections.iter().any(|other| {
                    (section.rva as u64) < other.virtual_end()
                        && (other.rva as u64) < section.virtual_end()
                })
            {
                return Err("overlapping or overflowing PE virtual sections".into());
            }
            sections.push(section);
        }
        // This also prevents forwarder strings from escaping the export directory.
        let directory = from_rva(bytes, &sections, export_rva, export_size as usize)?;
        let ordinal_base = u32_at(bytes, directory + 16)?;
        let functions = u32_at(bytes, directory + 20)? as usize;
        let names = u32_at(bytes, directory + 24)? as usize;
        if functions == 0
            || functions > 65536
            || names > 65536
            || ordinal_base.checked_add(functions as u32 - 1).is_none()
        {
            return Err("invalid PE export counts or ordinal range".into());
        }
        let addresses_offset = from_rva(
            bytes,
            &sections,
            u32_at(bytes, directory + 28)?,
            functions * 4,
        )?;
        let mut addresses = Vec::with_capacity(functions);
        for index in 0..functions {
            addresses.push(u32_at(bytes, addresses_offset + index * 4)?);
        }
        let mut exports = HashMap::with_capacity(names);
        if names != 0 {
            let name_pointers =
                from_rva(bytes, &sections, u32_at(bytes, directory + 32)?, names * 4)?;
            let ordinals = from_rva(bytes, &sections, u32_at(bytes, directory + 36)?, names * 2)?;
            for index in 0..names {
                let name_rva = u32_at(bytes, name_pointers + index * 4)?;
                let name_offset = from_rva(bytes, &sections, name_rva, 1)?;
                let length = bytes[name_offset..]
                    .iter()
                    .take(4097)
                    .position(|&b| b == 0)
                    .ok_or("unterminated PE export name")?;
                if length == 0 {
                    return Err("empty PE export name".into());
                }
                from_rva(bytes, &sections, name_rva, length + 1)?;
                let name = std::str::from_utf8(range(bytes, name_offset, length)?)?;
                let ordinal = u16_at(bytes, ordinals + index * 2)? as usize;
                if ordinal >= functions {
                    return Err("PE export ordinal exceeds address table".into());
                }
                if exports.insert(name, ordinal).is_some() {
                    return Err("duplicate PE export name".into());
                }
            }
        }
        Ok(Self {
            bytes,
            sections,
            image_base,
            export_rva,
            export_end,
            ordinal_base,
            addresses,
            names: exports,
        })
    }

    fn validate_lifecycle(&self, module: &Module) -> Result<()> {
        for name in MANAGEMENT_EXPORTS {
            match module.lifecycle {
                ModuleLifecycle::None if self.names.contains_key(name) => {
                    return Err(format!(
                        "stateless module {} exports management ABI {name}",
                        module.soname
                    )
                    .into());
                }
                ModuleLifecycle::None => {}
                ModuleLifecycle::RuntimeApiV1 => {
                    let index = self.named(name)?;
                    if self.forwarded(index)?.is_some() {
                        return Err(format!(
                            "module {} forwards management ABI {name}",
                            module.soname
                        )
                        .into());
                    }
                    self.concrete(index, &Export::function(name, name))?;
                }
            }
        }
        Ok(())
    }

    fn named(&self, name: &str) -> Result<usize> {
        self.names
            .get(name)
            .copied()
            .ok_or_else(|| format!("PE DLL lacks required export {name}").into())
    }

    fn forwarded(&self, index: usize) -> Result<Option<Forwarder<'a>>> {
        let rva = self.addresses[index];
        if rva == 0 {
            return Err("null PE export address".into());
        }
        if !(self.export_rva..self.export_end).contains(&rva) {
            return Ok(None);
        }
        let maximum = ((self.export_end - rva) as usize).min(4097);
        let offset = from_rva(self.bytes, &self.sections, rva, maximum)?;
        let length = range(self.bytes, offset, maximum)?
            .iter()
            .position(|&b| b == 0)
            .ok_or("unterminated PE forwarder inside export directory")?;
        let value = std::str::from_utf8(range(self.bytes, offset, length)?)?;
        Ok(Some(parse_forwarder(value)?))
    }

    fn concrete(&self, index: usize, export: &Export) -> Result<()> {
        let rva = self.addresses[index];
        let section = self
            .sections
            .iter()
            .find(|section| rva >= section.rva && (rva as u64) < section.virtual_end())
            .ok_or("PE export is not contained in a virtual section")?;
        match export.kind {
            ExportKind::Function => {
                if section.flags & EXECUTE == 0 {
                    return Err(
                        format!("{} is not in an executable PE section", export.pe_export).into(),
                    );
                }
                from_rva(self.bytes, &self.sections, rva, 1)?;
            }
            ExportKind::Object => {
                if export.size == 0 || !export.alignment.is_power_of_two() {
                    return Err("PE object requires nonzero size and power-of-two alignment".into());
                }
                if section.flags & READ == 0 || section.flags & EXECUTE != 0 {
                    return Err(format!(
                        "{} is not in a readable, nonexecutable PE data section",
                        export.pe_export
                    )
                    .into());
                }
                let address = self
                    .image_base
                    .checked_add(rva as u64)
                    .ok_or("PE object address overflow")?;
                if address % export.alignment != 0 {
                    return Err("PE object address violates declared alignment".into());
                }
                // PE maps and zero-fills the virtual tail of a data section. BSS
                // objects need virtual backing, not bytes in the on-disk image.
                let end = (rva as u64)
                    .checked_add(export.size)
                    .ok_or("PE object range overflow")?;
                if end > section.virtual_end() {
                    return Err("PE object crosses its virtual data section boundary".into());
                }
                address
                    .checked_add(export.size)
                    .ok_or("PE object address range overflow")?;
            }
        }
        Ok(())
    }
}

/// Validate one DLL without loading it. Forwarders are checked for well-formed syntax;
/// only `validate_package_exports` can prove that their concrete targets are valid.
#[allow(dead_code)] // Retained for standalone diagnostics and malformed-PE unit tests.
pub fn validate_exports(bytes: &[u8], module: &Module) -> Result<()> {
    let pe = Pe::parse(bytes, &module.soname)?;
    for export in &module.exports {
        let index = pe.named(&export.pe_export)?;
        if pe.forwarded(index)?.is_none() {
            pe.concrete(index, export)?;
        }
    }
    Ok(())
}

/// Resolve every required export strictly within this package's native images.
/// A forwarder does not change the required ELF kind, object size, or alignment.
#[cfg(test)]
pub fn validate_package_exports(pairs: &[(&Module, &[u8])]) -> Result<()> {
    validate_package_with_dependencies(pairs, &[])
}

pub fn validate_package_with_dependencies(
    pairs: &[(&Module, &[u8])],
    dependencies: &[(&str, &[u8])],
) -> Result<()> {
    let mut names = HashMap::with_capacity(pairs.len());
    let mut images = Vec::with_capacity(pairs.len());
    for (index, (module, bytes)) in pairs.iter().enumerate() {
        if names.insert(module_key(&module.soname)?, index).is_some() {
            return Err("duplicate case-insensitive DLL identity in PE package".into());
        }
        let image = Pe::parse(bytes, &module.soname)?;
        image.validate_lifecycle(module)?;
        images.push(image);
    }
    for (name, bytes) in dependencies {
        if names.insert(module_key(name)?, images.len()).is_some() {
            return Err("duplicate packaged native dependency".into());
        }
        images.push(Pe::parse(bytes, name)?);
    }
    // Rust native libraries from different feature/build graphs can share a
    // filename but have different symbol hashes. Reject a mixed build before
    // publication instead of discovering it as Win32 ERROR_PROC_NOT_FOUND.
    for image in &images {
        let header = u32_at(image.bytes, 0x3c)? as usize + 24;
        let rva = u32_at(image.bytes, header + 120)?;
        let size = u32_at(image.bytes, header + 124)? as usize;
        if rva == 0 {
            continue;
        }
        let start = from_rva(image.bytes, &image.sections, rva, size)?;
        for relative in (0..size).step_by(20) {
            let descriptor = start + relative;
            if range(image.bytes, descriptor, 20)?.iter().all(|&b| b == 0) {
                break;
            }
            let name_rva = u32_at(image.bytes, descriptor + 12)?;
            let name_at = from_rva(image.bytes, &image.sections, name_rva, 1)?;
            let length = image.bytes[name_at..]
                .iter()
                .take(256)
                .position(|&b| b == 0)
                .ok_or("unterminated import module")?;
            let name = std::str::from_utf8(range(image.bytes, name_at, length)?)?;
            let Some(&target) = names.get(&module_key(name)?) else {
                continue;
            };
            let lookup = u32_at(image.bytes, descriptor)?;
            if lookup == 0 {
                return Err("owned native import lacks a lookup table".into());
            }
            for index in 0..65536u32 {
                let thunk_at = from_rva(
                    image.bytes,
                    &image.sections,
                    lookup
                        .checked_add(index * 8)
                        .ok_or("import table overflow")?,
                    8,
                )?;
                let thunk = u64_at(image.bytes, thunk_at)?;
                if thunk == 0 {
                    break;
                }
                let ordinal = if thunk & (1 << 63) != 0 {
                    (thunk as u16 as u32)
                        .checked_sub(images[target].ordinal_base)
                        .ok_or("invalid native import ordinal")? as usize
                } else {
                    let symbol_at =
                        from_rva(image.bytes, &image.sections, u32::try_from(thunk)? + 2, 1)?;
                    let length = image.bytes[symbol_at..]
                        .iter()
                        .take(4097)
                        .position(|&b| b == 0)
                        .ok_or("unterminated import symbol")?;
                    let symbol = std::str::from_utf8(range(image.bytes, symbol_at, length)?)?;
                    images[target]
                        .named(symbol)
                        .map_err(|error| format!("native import {name}: {error}"))?
                };
                if images[target]
                    .addresses
                    .get(ordinal)
                    .is_none_or(|&address| address == 0)
                {
                    return Err("native import points to an absent export".into());
                }
            }
        }
    }
    for (source, (module, _)) in pairs.iter().enumerate() {
        for export in &module.exports {
            let mut current = source;
            let mut index = images[current].named(&export.pe_export)?;
            let mut visited = HashSet::new();
            loop {
                if !visited.insert((current, index)) {
                    return Err("cyclic PE export forwarder".into());
                }
                if visited.len() > MAX_FORWARD_DEPTH {
                    return Err("PE export forwarder chain exceeds limit".into());
                }
                let Some(forwarder) = images[current].forwarded(index)? else {
                    images[current].concrete(index, export)?;
                    break;
                };
                current = *names.get(&module_key(forwarder.module)?).ok_or_else(|| {
                    format!(
                        "forwarded DLL {} is not present in this package",
                        forwarder.module
                    )
                })?;
                index = match forwarder.symbol {
                    ForwardSymbol::Name(name) => images[current].named(name)?,
                    ForwardSymbol::Ordinal(ordinal) => {
                        let offset = ordinal
                            .checked_sub(images[current].ordinal_base)
                            .ok_or("forwarded PE ordinal is below export base")?
                            as usize;
                        if offset >= images[current].addresses.len() {
                            return Err("forwarded PE ordinal exceeds address table".into());
                        }
                        offset
                    }
                };
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    enum Target<'a> {
        Address(u32),
        Forward(&'a str),
    }
    fn put16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn fixture(exports: &[(&str, Target<'_>)]) -> Vec<u8> {
        assert!(exports.len() <= 16);
        let mut bytes = vec![0u8; 0xc00];
        bytes[..2].copy_from_slice(b"MZ");
        put32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        put16(&mut bytes, 0x84, 0x8664);
        put16(&mut bytes, 0x86, 3);
        put16(&mut bytes, 0x94, 0xf0);
        put16(&mut bytes, 0x96, 0x2000);
        put16(&mut bytes, 0x98, 0x20b);
        put32(&mut bytes, 0x98 + 108, 16);
        put32(&mut bytes, 0x98 + 112, 0x3000);
        put32(&mut bytes, 0x98 + 116, 0x600);
        for (index, (rva, file, size, flags)) in [
            (0x1000, 0x200, 0x200, READ | EXECUTE),
            (0x2000, 0x400, 0x200, READ | 0x8000_0000),
            (0x3000, 0x600, 0x600, READ),
        ]
        .into_iter()
        .enumerate()
        {
            let at = 0x188 + index * 40;
            put32(&mut bytes, at + 8, size);
            put32(&mut bytes, at + 12, rva);
            put32(&mut bytes, at + 16, size);
            put32(&mut bytes, at + 20, file);
            put32(&mut bytes, at + 36, flags);
        }
        put32(&mut bytes, 0x610, 10);
        put32(&mut bytes, 0x614, exports.len() as u32);
        put32(&mut bytes, 0x618, exports.len() as u32);
        put32(&mut bytes, 0x61c, 0x3040);
        put32(&mut bytes, 0x620, 0x3080);
        put32(&mut bytes, 0x624, 0x30c0);
        let mut name_at = 0x700;
        let mut forward_at = 0xa00;
        for (index, (name, target)) in exports.iter().enumerate() {
            put32(&mut bytes, 0x680 + index * 4, (name_at + 0x2a00) as u32);
            put16(&mut bytes, 0x6c0 + index * 2, index as u16);
            bytes[name_at..name_at + name.len()].copy_from_slice(name.as_bytes());
            name_at += name.len() + 1;
            let address = match target {
                Target::Address(rva) => *rva,
                Target::Forward(value) => {
                    let rva = (forward_at + 0x2a00) as u32;
                    bytes[forward_at..forward_at + value.len()].copy_from_slice(value.as_bytes());
                    forward_at += value.len() + 1;
                    rva
                }
            };
            put32(&mut bytes, 0x640 + index * 4, address);
        }
        bytes
    }

    fn module(dll: &str, name: &str, object: bool) -> Module {
        let mut module = Module {
            id: 0,
            soname: dll.into(),
            lifecycle: ModuleLifecycle::None,
            exports: vec![Export::function(name, name)],
        };
        module.lifecycle = ModuleLifecycle::None;
        module.soname = dll.into();
        module.exports.truncate(1);
        module.exports[0].pe_export = name.into();
        module.exports[0].kind = if object {
            ExportKind::Object
        } else {
            ExportKind::Function
        };
        module.exports[0].size = if object { 8 } else { 0 };
        module.exports[0].alignment = if object { 8 } else { 1 };
        module
    }

    #[test]
    fn lifecycle_requires_local_code_and_stateless_images_have_no_sdk_exports() {
        let mut managed = module("managed.dll", "value", false);
        managed.lifecycle = ModuleLifecycle::RuntimeApiV1;
        let mut exports = vec![("value", Target::Address(0x1000))];
        for name in MANAGEMENT_EXPORTS {
            assert!(validate_package_exports(&[(&managed, &fixture(&exports))]).is_err());
            exports.push((name, Target::Address(0x1000)));
        }
        assert!(validate_package_exports(&[(&managed, &fixture(&exports))]).is_ok());
        managed.lifecycle = ModuleLifecycle::None;
        assert!(validate_package_exports(&[(&managed, &fixture(&exports))]).is_err());
        managed.lifecycle = ModuleLifecycle::RuntimeApiV1;
        for target in [Target::Address(0x2000), Target::Forward("managed.value")] {
            exports[1].1 = target;
            assert!(validate_package_exports(&[(&managed, &fixture(&exports))]).is_err());
        }
    }

    #[test]
    fn malformed_pe_is_rejected_without_loading_it() {
        let module = module("a.dll", "value", false);
        for size in [0, 1, 2, 63, 64, 128, 1024] {
            let mut bytes = vec![0u8; size];
            if size >= 2 {
                bytes[..2].copy_from_slice(b"MZ");
            }
            if size >= 64 {
                bytes[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            assert!(validate_exports(&bytes, &module).is_err());
        }
    }

    #[test]
    fn direct_functions_and_readable_nonexecutable_objects_are_distinct() {
        let bytes = fixture(&[
            ("code", Target::Address(0x1000)),
            ("value", Target::Address(0x2000)),
        ]);
        assert!(validate_exports(&bytes, &module("a.dll", "code", false)).is_ok());
        assert!(validate_exports(&bytes, &module("a.dll", "value", true)).is_ok());
        assert!(validate_exports(&bytes, &module("a.dll", "code", true)).is_err());
        assert!(validate_exports(&bytes, &module("a.dll", "value", false)).is_err());
        let mut object = module("a.dll", "value", true);
        object.exports[0].size = 0x201;
        assert!(validate_exports(&bytes, &object).is_err());
        object.exports[0].size = 8;
        object.exports[0].alignment = 0x4000;
        assert!(validate_exports(&bytes, &object).is_err());
        let misaligned = fixture(&[("value", Target::Address(0x2001))]);
        assert!(validate_exports(&misaligned, &module("a.dll", "value", true)).is_err());
    }

    #[test]
    fn objects_allow_zero_filled_virtual_storage_with_checked_bounds() {
        let mut bytes = fixture(&[("value", Target::Address(0x2200))]);
        let data_section = 0x188 + 40;
        put32(&mut bytes, data_section + 8, 0x500);
        let mut object = module("a.dll", "value", true);
        assert!(validate_exports(&bytes, &object).is_ok());

        // The same object can span initialized bytes and the zero-filled tail.
        put32(&mut bytes, 0x640, 0x21f8);
        object.exports[0].size = 16;
        assert!(validate_exports(&bytes, &object).is_ok());

        put32(&mut bytes, 0x640, 0x24f8);
        assert!(validate_exports(&bytes, &object).is_err());
        object.exports[0].size = 8;
        assert!(validate_exports(&bytes, &object).is_ok());
        put32(&mut bytes, 0x640, 0x2500);
        assert!(validate_exports(&bytes, &object).is_err());

        // Entirely uninitialized sections are also valid object storage.
        put32(&mut bytes, data_section + 16, 0);
        put32(&mut bytes, 0x640, 0x2000);
        assert!(validate_exports(&bytes, &object).is_ok());
        object.exports[0].size = u64::MAX;
        assert!(validate_exports(&bytes, &object).is_err());
    }

    #[test]
    fn forwarders_resolve_names_ordinals_chains_and_data() {
        for target in ["B.value", "b.dll.value", "B.#11"] {
            let a = fixture(&[("source", Target::Forward(target))]);
            let b = fixture(&[
                ("unused", Target::Address(0x1000)),
                ("value", Target::Forward("c.target")),
            ]);
            let c = fixture(&[("target", Target::Address(0x2000))]);
            let ma = module("a.dll", "source", true);
            let mb = module("b.dll", "value", true);
            let mc = module("c.dll", "target", true);
            assert!(
                validate_package_exports(&[(&ma, &a), (&mb, &b), (&mc, &c)]).is_ok(),
                "{target}"
            );
        }
        let a = fixture(&[("source", Target::Forward("b.target"))]);
        let b = fixture(&[("target", Target::Address(0x1000))]);
        assert!(
            validate_package_exports(&[
                (&module("a.dll", "source", false), &a),
                (&module("b.dll", "target", false), &b)
            ])
            .is_ok()
        );
        assert!(
            validate_package_exports(&[
                (&module("a.dll", "source", true), &a),
                (&module("b.dll", "target", false), &b)
            ])
            .is_err()
        );
    }

    #[test]
    fn malformed_missing_null_and_cyclic_forwarders_are_rejected() {
        let ma = module("a.dll", "source", false);
        for target in [
            "",
            "b",
            ".name",
            "b.",
            "../b.name",
            "b/name.x",
            "b.#",
            "b.#-1",
            "b.#0",
            "b.#65536",
            "b.#42949672960",
            "b.hello world",
            "b.#1x",
        ] {
            let bytes = fixture(&[("source", Target::Forward(target))]);
            assert!(validate_exports(&bytes, &ma).is_err(), "{target}");
        }
        let missing = fixture(&[("source", Target::Forward("outside.name"))]);
        assert!(validate_package_exports(&[(&ma, &missing)]).is_err());
        let a = fixture(&[("source", Target::Forward("b.target"))]);
        let mb = module("b.dll", "target", false);
        for target in [Target::Forward("a.source"), Target::Address(0)] {
            let b = fixture(&[("target", target)]);
            assert!(validate_package_exports(&[(&ma, &a), (&mb, &b)]).is_err());
        }
        for target in ["b.#9", "b.#11", "b.absent"] {
            let a = fixture(&[("source", Target::Forward(target))]);
            let b = fixture(&[("target", Target::Address(0x1000))]);
            assert!(validate_package_exports(&[(&ma, &a), (&mb, &b)]).is_err());
        }
        let mut unterminated = fixture(&[("source", Target::Forward("b.target"))]);
        unterminated[0xa00..].fill(b'x');
        assert!(validate_exports(&unterminated, &ma).is_err());
        let mut bounded = fixture(&[("source", Target::Forward("b.target"))]);
        put32(&mut bounded, 0x98 + 116, 0x405); // Terminator is outside directory.
        assert!(validate_exports(&bounded, &ma).is_err());
    }
}
