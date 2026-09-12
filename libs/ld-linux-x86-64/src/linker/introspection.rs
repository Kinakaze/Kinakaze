//! Stable GNU link_map views of the actual runtime loader objects.

use super::*;
use std::ffi::{CString, c_char, c_void};
use std::ptr;

#[repr(C)]
pub struct GuestLinkMap {
    pub l_addr: usize,
    pub l_name: *const c_char,
    pub l_ld: *mut c_void,
    pub l_next: *mut GuestLinkMap,
    pub l_prev: *mut GuestLinkMap,
}

pub struct RuntimeObjectInfo {
    pub map: GuestMap,
    pub origin: CString,
    pub program_headers: usize,
    pub program_count: u16,
    pub tls_module: Option<usize>,
    pub search_paths: Vec<CString>,
}

/// Only the C record and its name escape to guest code. Their addresses survive
/// fork; the enclosing Rust metadata is serialized and rebuilt privately.
pub struct GuestMap {
    pointer: *mut GuestLinkMap,
    length: usize,
}

impl GuestMap {
    fn new(mut record: GuestLinkMap, name: &std::ffi::CStr) -> Result<Self, LinkError> {
        let length = std::mem::size_of::<GuestLinkMap>()
            .checked_add(name.to_bytes_with_nul().len())
            .ok_or(LinkError::AddressOverflow)?;
        let pointer = unsafe {
            kinakaze_alloc::reallocate(
                ptr::null_mut(),
                std::mem::align_of::<GuestLinkMap>(),
                length,
            )
        }
        .cast::<GuestLinkMap>();
        if pointer.is_null() {
            return Err(LinkError::AddressOverflow);
        }
        let name_pointer = unsafe { pointer.add(1).cast::<u8>() };
        record.l_name = name_pointer.cast();
        unsafe {
            ptr::copy_nonoverlapping(
                name.to_bytes_with_nul().as_ptr(),
                name_pointer,
                name.to_bytes_with_nul().len(),
            );
            pointer.write(record);
        }
        Ok(Self { pointer, length })
    }
}
impl std::ops::Deref for GuestMap {
    type Target = GuestLinkMap;
    fn deref(&self) -> &GuestLinkMap {
        unsafe { &*self.pointer }
    }
}
impl std::ops::DerefMut for GuestMap {
    fn deref_mut(&mut self) -> &mut GuestLinkMap {
        unsafe { &mut *self.pointer }
    }
}
impl Drop for GuestMap {
    fn drop(&mut self) {
        unsafe { kinakaze_alloc::free(self.pointer.cast()) };
    }
}

// Pointers reference mappings and pinned strings owned by the same Linker.
// Its loader lock serializes every mutation and callers obey the link_map ABI.
unsafe impl Send for RuntimeObjectInfo {}

#[derive(Default)]
pub(super) struct RuntimeMaps {
    entries: HashMap<RuntimeHandle, Box<RuntimeObjectInfo>>,
    active: Vec<RuntimeHandle>,
}

#[cfg(windows)]
impl RuntimeMaps {
    pub(super) fn snapshot(&self, out: &mut super::fork::Writer) {
        out.word(self.entries.len());
        for (target, info) in &self.entries {
            out.handle(*target);
            out.word(info.map.pointer as usize);
            out.word(info.map.length);
            out.bytes(info.origin.as_bytes());
            out.word(info.program_headers);
            out.word(info.program_count as usize);
            out.word(info.tls_module.unwrap_or(usize::MAX));
            out.word(info.search_paths.len());
            for path in &info.search_paths {
                out.bytes(path.as_bytes());
            }
        }
        out.word(self.active.len());
        for target in &self.active {
            out.handle(*target);
        }
    }

    pub(super) unsafe fn restore(input: &mut super::fork::Reader<'_>) -> Result<Self, LinkError> {
        let invalid = || LinkError::InvalidProvider("invalid fork link_map".into());
        let mut result = Self::default();
        let mut addresses = std::collections::HashSet::new();
        for _ in 0..input.count()? {
            let target = input.handle()?;
            let address = input.word()?;
            let length = input.word()?;
            if length <= std::mem::size_of::<GuestLinkMap>()
                || address % std::mem::align_of::<GuestLinkMap>() != 0
                || address < kinakaze_alloc::ARENA_BASE + 4096
                || address
                    .checked_add(length)
                    .is_none_or(|end| end > kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE)
                || !addresses.insert(address)
                || result.entries.contains_key(&target)
            {
                return Err(invalid());
            }
            let pointer = address as *mut GuestLinkMap;
            if unsafe { (*pointer).l_name as usize }
                != address + std::mem::size_of::<GuestLinkMap>()
            {
                return Err(invalid());
            }
            let origin = CString::new(input.bytes()?).map_err(|_| invalid())?;
            let program_headers = input.word()?;
            let program_count = u16::try_from(input.word()?).map_err(|_| invalid())?;
            let tls = input.word()?;
            let mut search_paths = Vec::new();
            for _ in 0..input.count()? {
                search_paths.push(CString::new(input.bytes()?).map_err(|_| invalid())?);
            }
            result.entries.insert(
                target,
                Box::new(RuntimeObjectInfo {
                    map: GuestMap { pointer, length },
                    origin,
                    program_headers,
                    program_count,
                    tls_module: (tls != usize::MAX).then_some(tls),
                    search_paths,
                }),
            );
        }
        for _ in 0..input.count()? {
            let target = input.handle()?;
            if !result.entries.contains_key(&target) || result.active.contains(&target) {
                return Err(invalid());
            }
            result.active.push(target);
        }
        Ok(result)
    }
}

fn guest_string(path: &Path) -> Result<CString, LinkError> {
    // Search paths mix native image directories with Linux LD_LIBRARY_PATH /
    // RUNPATH entries. A Linux path already belongs to the guest namespace:
    // native canonicalization can interpret it as a Windows network path and
    // block dlopen on an unrelated SMB lookup.
    let raw = path.to_string_lossy();
    let guest = if raw.starts_with('/') {
        raw.replace('\\', "/")
    } else {
        kinakaze_vfs::to_guest_path(path)
    };
    CString::new(guest).map_err(|_| LinkError::InvalidProvider("loader path contains NUL".into()))
}

impl Linker {
    /// Describe a real PT_LOAD mapping and its PT_GNU_EH_FRAME header. GCC 12+
    /// uses this interface instead of dl_iterate_phdr for DWARF unwinding.
    pub fn find_object_info(
        &mut self,
        address: usize,
    ) -> Result<Option<kinakaze_runtime::services::FindObjectInfo>, LinkError> {
        self.refresh_runtime_maps()?;
        for target in &self.runtime_maps.active {
            let info = &self.runtime_maps.entries[target];
            let mut start = usize::MAX;
            let mut end = 0usize;
            let mut contains = false;
            let mut eh_frame = 0usize;
            for index in 0..usize::from(info.program_count) {
                // All records are validated, lifetime-pinned Elf64_Phdr arrays.
                let entry = (info.program_headers + index * 56) as *const u8;
                let kind = unsafe { ptr::read_unaligned(entry.cast::<u32>()) };
                let virtual_address = unsafe { ptr::read_unaligned(entry.add(16).cast::<u64>()) };
                let base = info.map.l_addr.wrapping_add(virtual_address as usize);
                if kind == kinakaze_elf::PT_LOAD {
                    let size = unsafe { ptr::read_unaligned(entry.add(40).cast::<u64>()) };
                    let limit = base
                        .checked_add(size as usize)
                        .ok_or(LinkError::AddressOverflow)?;
                    start = start.min(base);
                    end = end.max(limit);
                    contains |= base <= address && address < limit;
                } else if kind == 0x6474_e550 {
                    eh_frame = base;
                }
            }
            if contains {
                return Ok(Some(kinakaze_runtime::services::FindObjectInfo {
                    flags: 0,
                    map_start: start,
                    map_end: end,
                    link_map: info.map.pointer as usize,
                    eh_frame,
                    reserved: [0; 7],
                }));
            }
        }
        Ok(None)
    }

    /// Publish pinned link_map records in the same order as symbol lookup.
    /// Existing record addresses and names survive vector growth and dlopen.
    pub fn refresh_runtime_maps(&mut self) -> Result<(), LinkError> {
        let active = self
            .scope
            .providers
            .iter()
            .filter_map(|provider| match *provider {
                Provider::Elf(id) => Some(RuntimeHandle::Elf(id)),
                Provider::Dll(index)
                    if self
                        .scope
                        .dlls
                        .get(index)
                        .is_some_and(|dll| dll.is_loaded()) =>
                {
                    Some(RuntimeHandle::Dll(index))
                }
                Provider::Dll(_) => None,
            })
            .collect::<Vec<_>>();
        for target in &active {
            if self.runtime_maps.entries.contains_key(target) {
                continue;
            }
            let (name, directory, bias, dynamic, phdr, phnum, tls, runpath) = match *target {
                RuntimeHandle::Elf(id) => {
                    let object = self.objects.get(id.0).ok_or(LinkError::InvalidHandle)?;
                    let elf = object.elf()?;
                    let headers = elf.program_headers()?;
                    let header = elf.header();
                    let phdr_size =
                        u64::from(header.program_count) * u64::from(header.program_entry_size);
                    let phdr = headers
                        .iter()
                        .find(|entry| entry.kind == kinakaze_elf::PT_PHDR)
                        .map(|entry| object.resolve_address(entry.virtual_address))
                        .transpose()?
                        .or_else(|| {
                            headers
                                .iter()
                                .find(|entry| {
                                    entry.kind == kinakaze_elf::PT_LOAD
                                        && entry.offset <= header.program_offset
                                        && header.program_offset.checked_add(phdr_size).is_some_and(
                                            |end| {
                                                end <= entry.offset.saturating_add(entry.file_size)
                                            },
                                        )
                                })
                                .and_then(|entry| {
                                    object
                                        .resolve_address(
                                            entry.virtual_address + header.program_offset
                                                - entry.offset,
                                        )
                                        .ok()
                                })
                        })
                        .unwrap_or(0);
                    let dynamic = headers
                        .iter()
                        .find(|entry| entry.kind == kinakaze_elf::PT_DYNAMIC)
                        .map(|entry| object.resolve_address(entry.virtual_address))
                        .transpose()?
                        .unwrap_or(0);
                    let directory = object
                        .path
                        .parent()
                        .unwrap_or(&self.paths.host_directory)
                        .to_path_buf();
                    let runpath = elf
                        .dynamic_search_path(&object.dynamic)?
                        .map(|raw| {
                            super::expand_search_path(raw, &directory, &self.paths.host_directory)
                        })
                        .unwrap_or_default();
                    (
                        if object.is_executable {
                            CString::default()
                        } else {
                            guest_string(&object.path)?
                        },
                        directory,
                        object.load_bias as usize,
                        dynamic,
                        phdr,
                        header.program_count,
                        object.tls_module,
                        runpath,
                    )
                }
                RuntimeHandle::Dll(index) => {
                    let image = self.scope.dlls[index].image();
                    let (phdr, phnum) = image.program_headers().unwrap_or((0, 0));
                    let mut dynamic = 0;
                    // ProviderImage contract pins validated ELF program headers.
                    if phdr != 0 {
                        for index in 0..usize::from(phnum) {
                            let entry = (phdr + index * 56) as *const u8;
                            let kind = unsafe { ptr::read_unaligned(entry.cast::<u32>()) };
                            if kind == kinakaze_elf::PT_DYNAMIC {
                                let address =
                                    unsafe { ptr::read_unaligned(entry.add(16).cast::<u64>()) };
                                dynamic = image
                                    .base()
                                    .checked_add(address as usize)
                                    .ok_or(LinkError::AddressOverflow)?;
                                break;
                            }
                        }
                    }
                    (
                        guest_string(image.path())?,
                        image
                            .path()
                            .parent()
                            .unwrap_or(&self.paths.host_directory)
                            .to_path_buf(),
                        image.base(),
                        dynamic,
                        phdr,
                        phnum,
                        None,
                        Vec::new(),
                    )
                }
                RuntimeHandle::Main => unreachable!(),
            };
            let object_system_dirs = [directory.join("../lib"), directory.join("../usr/lib")];
            let search_paths = self
                .library_paths
                .iter()
                .chain(runpath.iter())
                .chain(self.paths.extra.iter())
                .chain(object_system_dirs.iter())
                .chain(self.system_paths.iter())
                .chain(std::iter::once(&directory))
                .map(|path| guest_string(path))
                .collect::<Result<Vec<_>, _>>()?;
            let info = Box::new(RuntimeObjectInfo {
                map: GuestMap::new(
                    GuestLinkMap {
                        l_addr: bias,
                        l_name: ptr::null(),
                        l_ld: dynamic as _,
                        l_next: ptr::null_mut(),
                        l_prev: ptr::null_mut(),
                    },
                    &name,
                )?,
                origin: guest_string(&directory)?,
                program_headers: phdr,
                program_count: phnum,
                tls_module: tls,
                search_paths,
            });
            self.runtime_maps.entries.insert(*target, info);
        }
        let addresses = active
            .iter()
            .map(|target| {
                self.runtime_maps
                    .entries
                    .get_mut(target)
                    .expect("published map")
                    .map
                    .pointer
            })
            .collect::<Vec<_>>();
        for (index, target) in active.iter().enumerate() {
            let entry = self
                .runtime_maps
                .entries
                .get_mut(target)
                .expect("published map");
            entry.map.l_prev = index
                .checked_sub(1)
                .map_or(ptr::null_mut(), |index| addresses[index]);
            entry.map.l_next = addresses.get(index + 1).copied().unwrap_or(ptr::null_mut());
        }
        self.runtime_maps.active = active;
        Ok(())
    }

    /// Resolve an opaque dlopen handle or a previously returned live link_map.
    /// RTLD_DEFAULT/RTLD_NEXT are dlsym selectors and are not valid dlinfo handles.
    pub fn runtime_object_info(&mut self, handle: usize) -> Result<&RuntimeObjectInfo, LinkError> {
        self.refresh_runtime_maps()?;
        let target = match self.runtime_handles.get(&handle).copied() {
            Some(RuntimeHandle::Main) => self
                .objects
                .iter()
                .position(|object| object.is_executable)
                .or_else(|| (!self.objects.is_empty()).then_some(0))
                .map(|index| RuntimeHandle::Elf(ObjectId(index)))
                .ok_or(LinkError::InvalidHandle)?,
            Some(target) => target,
            None => self
                .runtime_maps
                .active
                .iter()
                .copied()
                .find(|target| {
                    let entry = self
                        .runtime_maps
                        .entries
                        .get(target)
                        .expect("published map");
                    entry.map.pointer as usize == handle
                })
                .ok_or(LinkError::InvalidHandle)?,
        };
        if !self.runtime_maps.active.contains(&target) {
            return Err(LinkError::InvalidHandle);
        }
        self.runtime_maps
            .entries
            .get(&target)
            .map(Box::as_ref)
            .ok_or(LinkError::InvalidHandle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn guest_search_paths_stay_in_the_linux_namespace() {
        for (source, expected) in [
            ("/usr/lib", "/usr/lib"),
            ("//usr/lib", "//usr/lib"),
            ("/lib/x86_64-linux-gnu", "/lib/x86_64-linux-gnu"),
            ("/usr/lib\\plugins", "/usr/lib/plugins"),
            ("/usr/lib/\u{f03a}", "/usr/lib/\u{f03a}"),
        ] {
            assert_eq!(
                guest_string(Path::new(source)).unwrap().to_bytes(),
                expected.as_bytes()
            );
        }
    }

    struct Image {
        name: String,
        path: PathBuf,
        bytes: Box<[u8; 256]>,
    }
    impl Image {
        fn new(name: &str) -> Self {
            let mut bytes = Box::new([0u8; 256]);
            bytes[64..68].copy_from_slice(&kinakaze_elf::PT_LOAD.to_le_bytes());
            bytes[120..124].copy_from_slice(&kinakaze_elf::PT_DYNAMIC.to_le_bytes());
            bytes[136..144].copy_from_slice(&208u64.to_le_bytes());
            Self {
                name: name.into(),
                path: Path::new("/runtime-info").join(name),
                bytes,
            }
        }
    }
    impl crate::ProviderImage for Image {
        fn soname(&self) -> &str {
            &self.name
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn base(&self) -> usize {
            self.bytes.as_ptr() as usize
        }
        fn symbols(&self) -> &[crate::ProviderSymbol] {
            &[]
        }
        fn mapped_len(&self) -> usize {
            self.bytes.len()
        }
        fn program_headers(&self) -> Option<(usize, u16)> {
            Some((self.base() + 64, 2))
        }
        fn redirect_copy(&self, _: &str, _: usize, _: u64) -> Result<(), LinkError> {
            Err(LinkError::InvalidHandle)
        }
    }

    #[test]
    fn link_maps_are_stable_real_metadata_and_track_handle_lifetime() {
        let mut linker = Linker::new(SearchPaths::with_host_directory(PathBuf::from(
            "/runtime-info",
        )));
        let first: Arc<dyn crate::ProviderImage> = Arc::new(Image::new("libfirst.so.1"));
        let base = first.base();
        let first_index = linker.load_registered(first).unwrap();
        let first_target = RuntimeHandle::Dll(first_index);
        let handle = linker.allocate_runtime_handle(first_target);
        let info = linker.runtime_object_info(handle).unwrap();
        assert_eq!(info.map.l_addr, base);
        assert_eq!(info.map.l_ld as usize, base + 208);
        assert_eq!(info.program_headers, base + 64);
        assert_eq!(info.program_count, 2);
        assert_eq!(info.tls_module, None);
        assert_eq!(
            unsafe { std::ffi::CStr::from_ptr(info.map.l_name) }.to_bytes(),
            b"/runtime-info/libfirst.so.1"
        );
        let map = (&raw const *info.map) as usize;
        assert!(info.map.l_prev.is_null());
        assert!(info.map.l_next.is_null());

        let second: Arc<dyn crate::ProviderImage> = Arc::new(Image::new("libsecond.so.1"));
        let second_index = linker.load_registered(second).unwrap();
        let second_handle = linker.allocate_runtime_handle(RuntimeHandle::Dll(second_index));
        let next = (&raw const *linker.runtime_object_info(second_handle).unwrap().map) as usize;
        assert_eq!(
            linker.runtime_object_info(handle).unwrap().map.l_next as usize,
            next
        );
        assert_eq!(
            (&raw const *linker.runtime_object_info(handle).unwrap().map) as usize,
            map
        );
        assert_eq!(linker.runtime_object_info(map).unwrap().map.l_addr, base);
        assert_eq!(
            linker.runtime_object_info(next).unwrap().map.l_prev as usize,
            map
        );

        linker.retain_runtime_target(first_target).unwrap();
        let retained_handle = linker.allocate_runtime_handle(first_target);
        linker.close_runtime(handle).unwrap();
        assert!(matches!(
            linker.runtime_object_info(handle),
            Err(LinkError::InvalidHandle)
        ));
        assert_eq!(linker.runtime_object_info(map).unwrap().map.l_addr, base);
        linker.close_runtime(retained_handle).unwrap();
        assert!(matches!(
            linker.runtime_object_info(map),
            Err(LinkError::InvalidHandle)
        ));
        assert!(
            linker
                .runtime_object_info(second_handle)
                .unwrap()
                .map
                .l_prev
                .is_null()
        );
        assert!(matches!(
            linker.runtime_object_info(0),
            Err(LinkError::InvalidHandle)
        ));
        assert!(matches!(
            linker.runtime_object_info(usize::MAX),
            Err(LinkError::InvalidHandle)
        ));
    }
}
