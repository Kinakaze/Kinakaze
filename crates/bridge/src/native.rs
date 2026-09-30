//! Read normal PE names and exports. There is no module descriptor or sidecar.
use crate::{Export, ExportKind, Module, ModuleLifecycle, ModuleSet, Result, invalid};
use kinakaze_v2_host_win::{Library, ReadOnlyFile};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
mod snapshot;
pub use snapshot::CatalogSnapshot;

#[cfg(test)]
#[path = "native_tests.rs"]
mod export_tests;

#[cfg(test)]
#[path = "native_catalog_tests.rs"]
mod catalog_tests;

/// Portable archives keep native images outside the writable guest root, so a
/// caller can select any new root on its very first launch. Developer and older
/// distributions retain their original layout.
pub fn directory(dist: &Path) -> std::path::PathBuf {
    let native = dist.join("native");
    if native.is_dir() {
        native
    } else {
        dist.join("rootfs/lib")
    }
}

pub struct NativeExport {
    pub name: String,
    /// A forwarder has no section of its own.
    pub object: Option<bool>,
}

pub struct NativeExports {
    pub name: String,
    pub symbols: Vec<NativeExport>,
}

struct BorrowedExport<'a> {
    name: &'a str,
    object: Option<bool>,
}

struct BorrowedExports<'a> {
    name: &'a str,
    symbols: Vec<BorrowedExport<'a>>,
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
    let image = borrowed_exports(bytes)?;
    Ok(NativeExports {
        name: image.name.to_owned(),
        symbols: image
            .symbols
            .into_iter()
            .map(|symbol| NativeExport {
                name: symbol.name.to_owned(),
                object: symbol.object,
            })
            .collect(),
    })
}

/// Identify a command facade without loading its code. Build timestamps, Rust
/// exports and debug paths may change while its native command ABI stays valid.
pub fn matches_command_facade(bytes: &[u8], native_name: &str, functions: &[&str]) -> bool {
    !functions.is_empty()
        && borrowed_exports(bytes).is_ok_and(|image| {
            image.name == native_name
                && functions.iter().all(|name| {
                    image
                        .symbols
                        .binary_search_by_key(name, |symbol| symbol.name)
                        .is_ok_and(|index| image.symbols[index].object == Some(false))
                })
        })
}

// Validate every export, including Rust-only names. Discovery only needs to
// own the guest ABI subset; borrow the remaining names from the pinned file
// instead of allocating and then discarding thousands of mangled strings.
fn borrowed_exports(bytes: &[u8]) -> Result<BorrowedExports<'_>> {
    selected_exports::<true>(bytes)
}

fn selected_exports<const ALL: bool>(bytes: &[u8]) -> Result<BorrowedExports<'_>> {
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
    // Export names and tables normally share one section. Keep a hint only
    // for this immutable image inspection; every hit still checks its bounds.
    let backing_hint = std::cell::Cell::new(0usize);
    let backing = |rva: u32, size: usize| -> Result<(usize, usize)> {
        let (start, _, raw, raw_size, _) = sections[backing_hint.get()];
        if let Some(relative) = rva.checked_sub(start)
            && (relative as u64) + size as u64 <= raw_size as u64
        {
            return Ok((
                raw as usize + relative as usize,
                (raw_size - relative) as usize,
            ));
        }
        for (index, &(start, _, raw, raw_size, _)) in sections.iter().enumerate() {
            if let Some(relative) = rva.checked_sub(start)
                && (relative as u64) + size as u64 <= raw_size as u64
            {
                backing_hint.set(index);
                let at = raw as usize + relative as usize;
                return Ok((at, (raw_size - relative) as usize));
            }
        }
        Err(invalid("unbacked PE RVA"))
    };
    let offset = |rva: u32, size: usize| -> Result<usize> {
        let (at, _) = backing(rva, size)?;
        Ok(at)
    };
    let string = |rva: u32| -> Result<&str> {
        let (at, available) = backing(rva, 1)?;
        // Use the standard library's byte search for long Rust export names.
        // Keep the existing length cap and section check; discovery still
        // validates every name in each worker's pinned immutable image.
        let tail = &bytes[at..at + available.min(4097)];
        let name = std::ffi::CStr::from_bytes_until_nul(tail)
            .map_err(|_| invalid("unterminated PE string"))?;
        name.to_str().map_err(|_| invalid("non-UTF8 PE name"))
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
    let addresses = span(
        bytes,
        offset(dword(bytes, directory + 28)?, functions * 4)?,
        functions * 4,
    )?;
    let pointers = span(
        bytes,
        offset(dword(bytes, directory + 32)?, names * 4)?,
        names * 4,
    )?;
    let ordinals = span(
        bytes,
        offset(dword(bytes, directory + 36)?, names * 2)?,
        names * 2,
    )?;
    let mut symbols = Vec::with_capacity(if ALL { names } else { 3 });
    let mut previous = None;
    let mut address_section = 0;
    for (pointer, ordinal) in pointers.chunks_exact(4).zip(ordinals.chunks_exact(2)) {
        let name = string(u32::from_le_bytes(pointer.try_into().unwrap()))?;
        if previous.is_some_and(|old: &str| old >= name) {
            return Err(invalid("unsorted or duplicate PE exports"));
        }
        previous = Some(name);
        let ordinal = u16::from_le_bytes(ordinal.try_into().unwrap()) as usize;
        if ordinal >= functions {
            return Err(invalid("PE ordinal exceeds address table"));
        }
        let address =
            u32::from_le_bytes(addresses[ordinal * 4..ordinal * 4 + 4].try_into().unwrap());
        if address == 0 {
            return Err(invalid("null PE export"));
        }
        let object = if (rva..end).contains(&address) {
            None
        } else {
            let (start, end, _, _, _) = sections[address_section];
            if address < start || address as u64 >= end {
                address_section = sections
                    .iter()
                    .position(|&(start, end, _, _, _)| address >= start && (address as u64) < end)
                    .ok_or_else(|| invalid("export outside PE sections"))?;
            }
            let flags = sections[address_section].4;
            if flags & 0x4000_0000 == 0 {
                return Err(invalid("unreadable PE export"));
            }
            Some(flags & 0x2000_0000 == 0)
        };
        if ALL
            || matches!(
                name,
                "kinakaze_runtime_open_v1"
                    | "kinakaze_provider_initialize_v1"
                    | "kinakaze_module_object_v1"
            )
        {
            symbols.push(BorrowedExport { name, object });
        }
    }
    Ok(BorrowedExports { name, symbols })
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
            let image = selected_exports::<false>(&bytes)?;
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
    let catalog = ModuleCatalog::discover(directory)?;
    let mut set = ModuleSet {
        modules: Vec::with_capacity(catalog.modules.len()),
        shared_libraries: catalog.shared_libraries,
        libraries: Vec::new(),
        images: catalog
            .images
            .iter()
            .filter_map(|image| match &image.file {
                NativeBytes::File(file) => Some(Arc::clone(file)),
                NativeBytes::Snapshot { .. } => None,
            })
            .collect(),
    };
    for discovered in catalog.modules {
        let (module, library) = discovered.materialize()?;
        set.modules.push(module);
        set.libraries.extend(library);
    }
    set.validate()?;
    Ok(set)
}

/// Validated PE identities and pinned files. Guest declarations and object
/// layouts are materialized only when a provider is actually requested.
pub struct ModuleCatalog {
    pub modules: Vec<DiscoveredModule>,
    pub shared_libraries: Vec<String>,
    images: Vec<Arc<NativeImage>>,
}

pub struct DiscoveredModule {
    pub id: u32,
    pub soname: String,
    pub native_name: String,
    pub lifecycle: ModuleLifecycle,
    image: Arc<NativeImage>,
}

struct NativeImage {
    path: PathBuf,
    file: NativeBytes,
    // Offsets into the pinned immutable file, never borrowed pointers or owned
    // copies of Rust export names. Discovery validates the PE once; binding
    // consumes this guest-only index instead of rescanning every export.
    exports: Vec<IndexedExport>,
    has_layout: bool,
}

enum NativeBytes {
    File(Arc<ReadOnlyFile>),
    Snapshot {
        view: Arc<kinakaze_v2_host_win::ReadOnlySectionView>,
        pin: usize,
    },
}
impl std::ops::Deref for NativeBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::File(file) => file.as_slice(),
            Self::Snapshot { view, .. } => view.as_slice(),
        }
    }
}

struct IndexedExport {
    start: u32,
    length: u32,
    object: Option<bool>,
}

impl NativeImage {
    fn exports(&self) -> impl Iterator<Item = BorrowedExport<'_>> {
        self.exports.iter().map(|entry| {
            let start = entry.start as usize;
            let end = start + entry.length as usize;
            BorrowedExport {
                name: std::str::from_utf8(&self.file[start..end])
                    .expect("validated, pinned PE export name"),
                object: entry.object,
            }
        })
    }
}

impl ModuleCatalog {
    pub fn discover(directory: &Path) -> Result<Self> {
        let directory = directory.canonicalize()?;
        let mut paths = fs::read_dir(&directory)?
            .map(|entry| {
                let entry = entry?;
                Ok((entry.path(), entry.file_type()?))
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort_by(|first, second| first.0.cmp(&second.0));
        let mut set = Self {
            modules: Vec::new(),
            shared_libraries: Vec::new(),
            images: Vec::new(),
        };
        for (path, file_type) in paths {
            let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_shared_object(filename)
                || !(file_type.is_file() || file_type.is_symlink() && path.is_file())
            {
                continue;
            }
            let bytes = read(&path)?;
            if bytes.starts_with(b"\x7fELF") {
                continue;
            } // The ELF linker owns ordinary guest libraries.
            let native = borrowed_exports(&bytes)?;
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
            let native_name = native.name.to_owned();
            let aliases: HashSet<_> = native
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
            let exports = if has_layout || runtime {
                native
                    .symbols
                    .iter()
                    .filter(|symbol| {
                        guest_symbol(symbol.name)
                            || aliases.contains(symbol.name)
                            || runtime && symbol.name == "kinakaze_runtime_abi_version"
                    })
                    .map(|symbol| IndexedExport {
                        // read() bounds the whole file to 128 MiB; each name was
                        // checked for UTF-8, section bounds and the 4096-byte cap.
                        start: (symbol.name.as_ptr() as usize - bytes.as_ptr() as usize) as u32,
                        length: symbol.name.len() as u32,
                        object: symbol.object,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let image = Arc::new(NativeImage {
                path: path.clone(),
                file: NativeBytes::File(Arc::new(bytes)),
                exports,
                has_layout,
            });
            set.images.push(Arc::clone(&image));
            if !has_layout && !runtime {
                set.shared_libraries.push(filename.to_owned());
                continue;
            }
            set.modules.push(DiscoveredModule {
                id: if runtime { 0 } else { module_id(filename) },
                soname: filename.into(),
                native_name,
                lifecycle,
                image,
            });
        }
        set.modules.sort_by_key(|module| module.id);
        if set.modules.is_empty() || set.modules.len() > crate::module::MAX_MODULES {
            return Err(invalid("module set must have 1..=128 modules"));
        }
        let mut names = HashSet::new();
        for name in &set.shared_libraries {
            crate::module::validate_filename(name)?;
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(invalid("invalid or duplicate shared native library"));
            }
        }
        let mut ids = HashSet::new();
        for module in &set.modules {
            crate::module::validate_filename(&module.soname)?;
            if !names.insert(module.soname.to_ascii_lowercase()) || !ids.insert(module.id) {
                return Err(invalid("duplicate module identity"));
            }
            if module.id == 0 && module.lifecycle != ModuleLifecycle::None {
                return Err(invalid("runtime cannot require provider initialization"));
            }
        }
        if !ids.contains(&0) {
            return Err(invalid("module set must include runtime module id 0"));
        }
        Ok(set)
    }

    /// Keep validation bytes alive across all deferred factories, including
    /// providers opened after fork. This operation does not load any DLLs.
    pub fn retain_images(&self) -> impl Send + Sync + 'static {
        self.images.clone()
    }
}

impl DiscoveredModule {
    /// Canonical identity of the inspected source, still protected by the
    /// catalog's file pin. Reuse that capability instead of reopening the path.
    pub fn canonical_path(&self) -> Result<PathBuf> {
        Ok(match &self.image.file {
            NativeBytes::File(file) => file.canonical_path()?,
            NativeBytes::Snapshot { view, pin } => view.pin_path(*pin)?,
        })
    }

    /// Validate complete guest declarations on demand. Transfer the optional
    /// layout-query DLL owner along with its metadata so it cannot unload in
    /// between querying an object's shape and binding the provider.
    pub fn materialize(&self) -> Result<(Module, Option<Arc<Library>>)> {
        // Reuse discovery's validated offsets into the same pinned file. The
        // live owner prevents mutation/unlink until deferred binding finishes.
        let filename = &self.soname;
        let has_layout = self.image.has_layout;
        type ObjectLayout = unsafe extern "C" fn(*const u8, usize) -> u64;
        let mut query: Option<ObjectLayout> = None;
        let mut owner = None;
        let mut declarations: Vec<_> = self
            .image
            .exports()
            .map(|symbol| {
                let (name, version) = symbol
                    .name
                    .split_once('@')
                    .map_or((symbol.name, None), |(name, version)| (name, Some(version)));
                (name, version, symbol.object)
            })
            .collect();
        // Group by the guest name, then the exact version. PE name order can
        // put `atan2` between `atan` and `atan@VERSION`; adjacent PE names alone
        // are not sufficient. Borrowed records avoid a tree node per export.
        declarations
            .sort_unstable_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(&right.1)));
        let mut symbols: Vec<Export> = Vec::with_capacity(declarations.len());
        for (name, version, object) in declarations {
            if let Some(export) = symbols.last_mut().filter(|export| export.name == name) {
                if let Some(version) = version {
                    export.versions.push(version.into());
                }
                continue;
            }
            let mut export = Export::function(name, name);
            if object != Some(false) && has_layout {
                let query = match query {
                    Some(query) => query,
                    None => {
                        let library = Arc::new(Library::open(&self.image.path)?);
                        // SAFETY: project module implements this fixed C ABI;
                        // the returned owner retains it for these declarations.
                        let function = unsafe {
                            std::mem::transmute::<*mut std::ffi::c_void, ObjectLayout>(
                                library.symbol(c"kinakaze_module_object_v1")?,
                            )
                        };
                        owner = Some(library);
                        query = Some(function);
                        function
                    }
                };
                let layout = unsafe { query(name.as_ptr(), name.len()) };
                if layout != 0 {
                    export.kind = if layout & (1 << 63) != 0 {
                        ExportKind::Tls
                    } else {
                        ExportKind::Object
                    };
                    export.size = layout as u32 as u64;
                    export.alignment = (layout & !(1 << 63)) >> 32;
                } else if object == Some(true) {
                    return Err(invalid(format!("missing object layout: {filename}:{name}")));
                }
            }
            if let Some(version) = version {
                export.versions.push(version.into());
            }
            symbols.push(export);
        }
        for export in &mut symbols {
            export.default_version = export.versions.last().cloned();
        }
        let module = Module {
            id: self.id,
            soname: self.soname.clone(),
            lifecycle: self.lifecycle,
            exports: symbols,
        };
        module.validate()?;
        Ok((module, owner))
    }
}
