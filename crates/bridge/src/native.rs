//! Read normal PE names and exports. There is no module descriptor or sidecar.
use crate::{Export, ExportKind, Module, ModuleLifecycle, ModuleSet, Result, invalid};
use kinakaze_v2_host_win::{Library, ReadOnlyFile};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

pub struct NativeExport {
    pub name: String,
    /// A forwarder has no section of its own.
    pub object: Option<bool>,
}

pub struct NativeExports {
    pub name: String,
    pub symbols: Vec<NativeExport>,
}

fn span(bytes: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    bytes
        .get(
            start
                ..start
                    .checked_add(length)
                    .ok_or_else(|| invalid("PE range overflow"))?,
        )
        .ok_or_else(|| invalid("PE range exceeds image"))
}

fn word(bytes: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(span(bytes, at, 2)?.try_into().unwrap()))
}

fn dword(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(span(bytes, at, 4)?.try_into().unwrap()))
}

/// Read-only inspection; no initializers are invoked by this operation.
pub fn exports(bytes: &[u8]) -> Result<NativeExports> {
    if span(bytes, 0, 2)? != b"MZ" {
        return Err(invalid("expected PE image"));
    }
    let nt = dword(bytes, 0x3c)? as usize;
    if span(bytes, nt, 4)? != b"PE\0\0"
        || word(bytes, nt + 4)? != 0x8664
        || word(bytes, nt + 22)? & 0x2000 == 0
    {
        return Err(invalid("expected x86-64 DLL"));
    }
    let optional = nt + 24;
    let optional_size = word(bytes, nt + 20)? as usize;
    span(bytes, optional, optional_size)?;
    if optional_size < 120 || word(bytes, optional)? != 0x20b || dword(bytes, optional + 108)? == 0
    {
        return Err(invalid("missing PE32+ export directory"));
    }
    let count = word(bytes, nt + 6)? as usize;
    if count == 0 || count > 96 {
        return Err(invalid("invalid PE section count"));
    }
    let mut sections = Vec::with_capacity(count);
    for index in 0..count {
        let at = optional + optional_size + index * 40;
        span(bytes, at, 40)?;
        let virtual_size = dword(bytes, at + 8)?;
        let rva = dword(bytes, at + 12)?;
        let raw_size = dword(bytes, at + 16)?;
        let raw = dword(bytes, at + 20)?;
        let flags = dword(bytes, at + 36)?;
        span(bytes, raw as usize, raw_size as usize)?;
        let end = (rva as u64) + virtual_size.max(raw_size) as u64;
        if end > u32::MAX as u64 + 1
            || sections
                .iter()
                .any(|&(start, end_old, _, _, _)| (rva as u64) < end_old && (start as u64) < end)
        {
            return Err(invalid("overlapping PE sections"));
        }
        sections.push((rva, end, raw, raw_size, flags));
    }
    let offset = |rva: u32, size: usize| -> Result<usize> {
        for &(start, _, raw, raw_size, _) in &sections {
            if let Some(relative) = rva.checked_sub(start)
                && (relative as u64) + size as u64 <= raw_size as u64
            {
                let at = raw as usize + relative as usize;
                span(bytes, at, size)?;
                return Ok(at);
            }
        }
        Err(invalid("unbacked PE RVA"))
    };
    let string = |rva: u32| -> Result<String> {
        let at = offset(rva, 1)?;
        let length = bytes[at..]
            .iter()
            .take(4097)
            .position(|&byte| byte == 0)
            .ok_or_else(|| invalid("unterminated PE string"))?;
        offset(rva, length + 1)?;
        Ok(std::str::from_utf8(&bytes[at..at + length])
            .map_err(|_| invalid("non-UTF8 PE name"))?
            .into())
    };
    let rva = dword(bytes, optional + 112)?;
    let length = dword(bytes, optional + 116)?;
    if rva == 0 || length < 40 {
        return Err(invalid("missing PE exports"));
    }
    let end = rva
        .checked_add(length)
        .ok_or_else(|| invalid("export directory overflow"))?;
    let directory = offset(rva, length as usize)?;
    let name = string(dword(bytes, directory + 12)?)?;
    let functions = dword(bytes, directory + 20)? as usize;
    let names = dword(bytes, directory + 24)? as usize;
    if functions == 0 || functions > 65536 || names > 65536 {
        return Err(invalid("invalid PE export counts"));
    }
    let addresses = offset(dword(bytes, directory + 28)?, functions * 4)?;
    let pointers = offset(dword(bytes, directory + 32)?, names * 4)?;
    let ordinals = offset(dword(bytes, directory + 36)?, names * 2)?;
    let mut symbols = Vec::with_capacity(names);
    let mut previous = None;
    for index in 0..names {
        let name = string(dword(bytes, pointers + index * 4)?)?;
        if previous.as_ref().is_some_and(|old: &String| old >= &name) {
            return Err(invalid("unsorted or duplicate PE exports"));
        }
        previous = Some(name.clone());
        let ordinal = word(bytes, ordinals + index * 2)? as usize;
        if ordinal >= functions {
            return Err(invalid("PE ordinal exceeds address table"));
        }
        let address = dword(bytes, addresses + ordinal * 4)?;
        if address == 0 {
            return Err(invalid("null PE export"));
        }
        let object = if (rva..end).contains(&address) {
            None
        } else {
            let flags = sections
                .iter()
                .find(|&&(start, end, _, _, _)| address >= start && (address as u64) < end)
                .ok_or_else(|| invalid("export outside PE sections"))?
                .4;
            if flags & 0x4000_0000 == 0 {
                return Err(invalid("unreadable PE export"));
            }
            Some(flags & 0x2000_0000 == 0)
        };
        symbols.push(NativeExport { name, object });
    }
    Ok(NativeExports { name, symbols })
}

pub fn read(path: &Path) -> Result<ReadOnlyFile> {
    Ok(ReadOnlyFile::open(path, 128 * 1024 * 1024)?)
}

pub fn is_shared_object(name: &str) -> bool {
    name.ends_with(".so") || name.contains(".so.")
}

/// Find the owning runtime without discovering every provider twice. The
/// runtime performs complete provider validation after opening its session.
pub fn runtime_path(directory: &Path) -> Result<std::path::PathBuf> {
    let directory = directory.canonicalize()?;
    let conventional = directory.join("libkinakaze-runtime.so.1");
    if conventional.is_file() {
        let bytes = read(&conventional)?;
        if !bytes.starts_with(b"\x7fELF") {
            let image = exports(&bytes)?;
            if image
                .symbols
                .iter()
                .any(|symbol| symbol.name == "kinakaze_runtime_open_v1")
            {
                return Ok(conventional);
            }
        }
    }
    // Keep discovery-based packages and renamed runtime images compatible.
    let modules = discover(&directory)?;
    let runtime = modules
        .modules
        .iter()
        .find(|module| module.id == 0)
        .ok_or_else(|| invalid("missing runtime module"))?;
    Ok(directory.join(&runtime.soname))
}

/// COFF import libraries already declare the filename used by dependent DLLs.
/// Read that standard linker output instead of maintaining a packaging map.
pub fn import_name(bytes: &[u8]) -> Result<String> {
    if span(bytes, 0, 8)? != b"!<arch>\n" {
        return Err(invalid("invalid COFF archive"));
    }
    let mut at = 8;
    let mut owner: Option<String> = None;
    while at < bytes.len() {
        let header = span(bytes, at, 60)?;
        if &header[58..] != b"`\n" {
            return Err(invalid("invalid COFF member header"));
        }
        let size: usize = std::str::from_utf8(&header[48..58])
            .map_err(|_| invalid("invalid COFF size"))?
            .trim()
            .parse()
            .map_err(|_| invalid("invalid COFF member size"))?;
        let member = span(bytes, at + 60, size)?;
        if member.starts_with(&[0, 0, 255, 255]) && member.len() >= 20 && word(member, 4)? == 0 {
            if word(member, 6)? != 0x8664 {
                return Err(invalid("non-x64 COFF import"));
            }
            let strings = span(member, 20, dword(member, 12)? as usize)?;
            let first = strings
                .iter()
                .position(|&byte| byte == 0)
                .ok_or_else(|| invalid("unterminated import symbol"))?
                + 1;
            let end = strings[first..]
                .iter()
                .position(|&byte| byte == 0)
                .ok_or_else(|| invalid("unterminated import DLL"))?
                + first;
            let name = std::str::from_utf8(&strings[first..end])
                .map_err(|_| invalid("invalid import DLL name"))?;
            crate::module::validate_filename(name)?;
            if name.is_empty() || owner.as_ref().is_some_and(|previous| previous != name) {
                return Err(invalid("ambiguous COFF import library owner"));
            }
            owner = Some(name.into());
        }
        span(bytes, at + 60 + size, size & 1)?;
        at = at
            .checked_add(60 + size + (size & 1))
            .ok_or_else(|| invalid("COFF member overflow"))?;
    }
    owner.ok_or_else(|| invalid("COFF archive contains no imports"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import_library(owner: &str) -> Vec<u8> {
        let strings = format!("entry\0{owner}\0").into_bytes();
        let mut member = vec![0u8; 20];
        member[..4].copy_from_slice(&[0, 0, 255, 255]);
        member[6..8].copy_from_slice(&0x8664u16.to_le_bytes());
        member[12..16].copy_from_slice(&(strings.len() as u32).to_le_bytes());
        member.extend(strings);
        let mut header = [b' '; 60];
        header[..6].copy_from_slice(b"entry/");
        let length = member.len().to_string();
        header[48..48 + length.len()].copy_from_slice(length.as_bytes());
        header[58..].copy_from_slice(b"`\n");
        let mut bytes = b"!<arch>\n".to_vec();
        bytes.extend(header);
        bytes.extend(&member);
        if member.len() & 1 != 0 {
            bytes.push(b'\n');
        }
        bytes
    }

    #[test]
    fn import_library_name_is_bounded_and_cannot_escape_the_package() {
        let bytes = import_library("libmodule.so.1");
        assert_eq!(import_name(&bytes).unwrap(), "libmodule.so.1");
        for length in 0..bytes.len() {
            assert!(import_name(&bytes[..length]).is_err(), "{length}");
        }
        for name in ["../other.so", "x:stream.so", "CON.so", "", "x\\other.so"] {
            assert!(import_name(&import_library(name)).is_err(), "{name}");
        }
        let mut mixed = bytes.clone();
        mixed.extend_from_slice(&import_library("libother.so")[8..]);
        assert!(import_name(&mixed).is_err());
    }
}

fn guest_symbol(name: &str) -> bool {
    !name.starts_with("kinakaze_")
        && !name.starts_with("_ZN")
        && !name.starts_with("_R")
        && !name.starts_with("__rust")
        && !name.starts_with("rust_")
}

fn module_id(name: &str) -> u32 {
    // Stable identity for init's state namespace, independent of directory order.
    name.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    })
}

pub(crate) fn discover(directory: &Path) -> Result<ModuleSet> {
    let directory = directory.canonicalize()?;
    let mut paths = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut set = ModuleSet {
        modules: Vec::new(),
        shared_libraries: Vec::new(),
        libraries: Vec::new(),
    };
    for path in paths {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_shared_object(filename) || !path.is_file() {
            continue;
        }
        let bytes = read(&path)?;
        if bytes.starts_with(b"\x7fELF") {
            continue;
        } // The ELF linker owns ordinary guest libraries.
        let native = exports(&bytes)?;
        let runtime = native
            .symbols
            .iter()
            .any(|symbol| symbol.name == "kinakaze_runtime_open_v1");
        let lifecycle = if native
            .symbols
            .iter()
            .any(|symbol| symbol.name == "kinakaze_provider_initialize_v1")
        {
            ModuleLifecycle::RuntimeApiV1
        } else {
            ModuleLifecycle::None
        };
        let has_layout = native
            .symbols
            .iter()
            .any(|symbol| symbol.name == "kinakaze_module_object_v1");
        if !has_layout && !runtime {
            set.shared_libraries.push(filename.to_owned());
            continue;
        }
        let aliases: std::collections::HashSet<_> = native
            .symbols
            .iter()
            .filter_map(|symbol| {
                symbol
                    .name
                    .strip_prefix("kinakaze_engine_")?
                    .split_once('_')
                    .map(|(_, name)| name)
            })
            .collect();
        let mut library = None;
        let mut symbols: BTreeMap<String, Export> = BTreeMap::new();
        for symbol in native.symbols.iter().filter(|symbol| {
            guest_symbol(&symbol.name)
                || aliases.contains(symbol.name.as_str())
                || runtime && symbol.name == "kinakaze_runtime_abi_version"
        }) {
            let (name, version) = symbol
                .name
                .split_once('@')
                .map_or((symbol.name.as_str(), None), |(name, version)| {
                    (name, Some(version))
                });
            if let Some(export) = symbols.get_mut(name) {
                if let Some(version) = version {
                    export.versions.push(version.into());
                }
                continue;
            }
            let mut export = Export::function(name, name);
            if symbol.object != Some(false) && has_layout {
                if library.is_none() {
                    library = Some(Arc::new(Library::open(&path)?));
                }
                type ObjectLayout = unsafe extern "C" fn(*const u8, usize) -> u64;
                // SAFETY: project module implements the fixed, borrowed-buffer C ABI.
                let query: ObjectLayout = unsafe {
                    std::mem::transmute(
                        library
                            .as_ref()
                            .unwrap()
                            .symbol(c"kinakaze_module_object_v1")?,
                    )
                };
                let layout = unsafe { query(name.as_ptr(), name.len()) };
                if layout != 0 {
                    export.kind = ExportKind::Object;
                    export.size = layout as u32 as u64;
                    export.alignment = layout >> 32;
                } else if symbol.object == Some(true) {
                    return Err(invalid(format!("missing object layout: {filename}:{name}")));
                }
            }
            if let Some(version) = version {
                export.versions.push(version.into());
            }
            symbols.insert(name.into(), export);
        }
        for export in symbols.values_mut() {
            export.default_version = export.versions.last().cloned();
        }
        if let Some(library) = library {
            set.libraries.push(library);
        }
        set.modules.push(Module {
            id: if runtime { 0 } else { module_id(filename) },
            soname: filename.into(),
            lifecycle,
            exports: symbols.into_values().collect(),
        });
    }
    set.modules.sort_by_key(|module| module.id);
    set.validate()?;
    Ok(set)
}
