//! Address and program-header inspection of the live PE/ELF image set.
use super::{dynamic_loader_lock, loaded_linker};
use std::cell::RefCell;
use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;

#[derive(Default)]
struct DlAddrStrings {
    path: Option<CString>,
    symbol: Option<CString>,
}

thread_local! {
    static DLADDR_STRINGS: RefCell<DlAddrStrings> = const {
        RefCell::new(DlAddrStrings { path: None, symbol: None })
    };
}

pub use kinakaze_runtime::services::AddressInfo as DlInfo;

pub(super) unsafe extern "sysv64" fn find_object(
    address: *const core::ffi::c_void,
    result: *mut kinakaze_runtime::services::FindObjectInfo,
) -> i32 {
    if address.is_null() || result.is_null() {
        return -1;
    }
    let _guard = dynamic_loader_lock();
    let Some(linker) = (unsafe { loaded_linker().as_mut() }) else {
        return -1;
    };
    let Ok(Some(info)) = linker.find_object_info(address as usize) else {
        return -1;
    };
    unsafe { result.write(info) };
    0
}

#[repr(C)]
pub struct DlPhdrInfo {
    dlpi_addr: usize,
    dlpi_name: *const core::ffi::c_char,
    dlpi_phdr: *const core::ffi::c_void,
    dlpi_phnum: u16,
    _padding: [u8; 6],
    dlpi_adds: u64,
    dlpi_subs: u64,
    dlpi_tls_modid: usize,
    dlpi_tls_data: *mut core::ffi::c_void,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_dladdr(
    address: *const core::ffi::c_void,
    info: *mut DlInfo,
) -> i32 {
    if address.is_null() || info.is_null() {
        return 0;
    }
    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if !linker.is_null()
        // SAFETY: protected by the process loader lock.
        && let Some(id) = unsafe { (&*linker).owner_of_address(address as usize) }
    {
        let object = unsafe { &*linker }.objects().get(id.0).expect("owner id");
        let path = CString::new(kinakaze_vfs::to_guest_path(&object.path))
            .unwrap_or_else(|_| c"<invalid>".to_owned());
        let symbol = unsafe { &*linker }
            .symbol_for_address(address as usize)
            .ok()
            .flatten();
        let symbol_address = symbol.as_ref().map_or(0, |(_, address)| *address);
        let symbol = symbol.and_then(|(name, _)| CString::new(name).ok());
        return DLADDR_STRINGS.with(|slot| {
            *slot.borrow_mut() = DlAddrStrings {
                path: Some(path),
                symbol,
            };
            let slot = slot.borrow();
            // SAFETY: info is validated and the path remains in TLS until the
            // next dladdr call on this thread.
            unsafe {
                (*info).dli_fname = slot.path.as_ref().expect("stored path").as_ptr();
                (*info).dli_fbase = object.allocation.cast();
                (*info).dli_sname = slot
                    .symbol
                    .as_ref()
                    .map_or(ptr::null(), |name| name.as_ptr());
                (*info).dli_saddr = symbol_address as *mut core::ffi::c_void;
            }
            1
        });
    }

    if !linker.is_null() {
        // SAFETY: the process loader lock also protects native image publication.
        let linker = unsafe { &*linker };
        let target = address as usize;
        for provider in linker.provider_images() {
            let symbol = provider.symbols().iter().find(|symbol| {
                symbol.address <= target
                    && symbol
                        .address
                        .checked_add(symbol.size.max(1) as usize)
                        .is_some_and(|end| target < end)
            });
            let inside = provider.base() <= target
                && provider
                    .base()
                    .checked_add(provider.mapped_len())
                    .is_some_and(|end| target < end);
            // A compatibility export may forward to another image. Attribute
            // an address to its actual mapping, not the first public alias.
            if !inside {
                continue;
            }
            let path = CString::new(kinakaze_vfs::to_guest_path(provider.path()))
                .unwrap_or_else(|_| c"<invalid>".to_owned());
            let symbol_address = symbol.map_or(0, |symbol| symbol.address);
            let name = symbol.and_then(|symbol| CString::new(symbol.name.as_str()).ok());
            return DLADDR_STRINGS.with(|slot| {
                *slot.borrow_mut() = DlAddrStrings {
                    path: Some(path),
                    symbol: name,
                };
                let slot = slot.borrow();
                // SAFETY: info was checked above and strings remain thread-local.
                unsafe {
                    (*info).dli_fname = slot.path.as_ref().expect("stored path").as_ptr();
                    (*info).dli_fbase = provider.base() as *mut core::ffi::c_void;
                    (*info).dli_sname = slot
                        .symbol
                        .as_ref()
                        .map_or(ptr::null(), |name| name.as_ptr());
                    (*info).dli_saddr = symbol_address as *mut core::ffi::c_void;
                }
                1
            });
        }
    }

    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    let mut module = ptr::null_mut();
    // SAFETY: FROM_ADDRESS interprets the second argument as an address.
    if unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address.cast(),
            &mut module,
        )
    } == 0
    {
        return 0;
    }
    let mut wide = vec![0u16; 32768];
    // SAFETY: module came from the loader and the buffer is writable.
    let length =
        unsafe { GetModuleFileNameW(module, wide.as_mut_ptr(), wide.len() as u32) } as usize;
    if length == 0 || length >= wide.len() {
        return 0;
    }
    use std::os::windows::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_wide(&wide[..length]));
    let path = CString::new(kinakaze_vfs::to_guest_path(&path))
        .unwrap_or_else(|_| c"<invalid>".to_owned());
    DLADDR_STRINGS.with(|slot| {
        *slot.borrow_mut() = DlAddrStrings {
            path: Some(path),
            symbol: None,
        };
        let slot = slot.borrow();
        // SAFETY: info is validated above and the path remains thread-local.
        unsafe {
            (*info).dli_fname = slot.path.as_ref().expect("stored path").as_ptr();
            (*info).dli_fbase = module.cast();
            (*info).dli_sname = ptr::null();
            (*info).dli_saddr = ptr::null_mut();
        }
        1
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_dl_iterate_phdr(
    callback: Option<
        unsafe extern "sysv64" fn(*mut DlPhdrInfo, usize, *mut core::ffi::c_void) -> i32,
    >,
    data: *mut core::ffi::c_void,
) -> i32 {
    let Some(callback) = callback else {
        return 0;
    };

    struct Snapshot {
        name: CString,
        info: DlPhdrInfo,
    }

    let snapshots = {
        let _guard = dynamic_loader_lock();
        let linker = loaded_linker();
        if linker.is_null() {
            return 0;
        }
        // SAFETY: protected by the process loader lock.
        let linker = unsafe { &*linker };
        let adds = (linker.objects().len() + linker.provider_images().count()) as u64;
        let mut snapshots = linker
            .objects()
            .iter()
            .filter_map(|object| {
                let elf = object.elf().ok()?;
                let header = elf.header();
                let headers = elf.program_headers().ok()?;
                let phdr_address = headers
                    .iter()
                    .find(|entry| entry.kind == kinakaze_elf::PT_PHDR)
                    .and_then(|entry| object.resolve_address(entry.virtual_address).ok())
                    .or_else(|| object.resolve_address(header.program_offset).ok())?;
                let name = CString::new(kinakaze_vfs::to_guest_path(&object.path)).ok()?;
                let tls_data = object
                    .tls_module
                    .and_then(|module| kinakaze_tls::elf_tls_get_addr(module, 0).ok())
                    .map_or(ptr::null_mut(), |address| address.cast());
                Some(Snapshot {
                    name,
                    info: DlPhdrInfo {
                        dlpi_addr: usize::try_from(object.load_bias).unwrap_or(0),
                        dlpi_name: ptr::null(),
                        dlpi_phdr: phdr_address as *const core::ffi::c_void,
                        dlpi_phnum: header.program_count,
                        _padding: [0; 6],
                        dlpi_adds: adds,
                        dlpi_subs: 0,
                        dlpi_tls_modid: object.tls_module.unwrap_or(0),
                        dlpi_tls_data: tls_data,
                    },
                })
            })
            .collect::<Vec<_>>();
        for provider in linker.provider_images() {
            let Some((headers, count)) = provider.program_headers() else {
                continue;
            };
            let Ok(name) = CString::new(kinakaze_vfs::to_guest_path(provider.path())) else {
                continue;
            };
            snapshots.push(Snapshot {
                name,
                info: DlPhdrInfo {
                    dlpi_addr: provider.base(),
                    dlpi_name: ptr::null(),
                    dlpi_phdr: headers as *const core::ffi::c_void,
                    dlpi_phnum: count,
                    _padding: [0; 6],
                    dlpi_adds: adds,
                    dlpi_subs: 0,
                    dlpi_tls_modid: 0,
                    dlpi_tls_data: ptr::null_mut(),
                },
            });
        }
        snapshots
    };

    // Do not hold the loader mutex across callbacks: callback code may call
    // dlopen itself, and the snapshot's mappings remain valid for this walk.
    for mut snapshot in snapshots {
        snapshot.info.dlpi_name = snapshot.name.as_ptr();
        // SAFETY: the callback follows the Linux dl_iterate_phdr ABI and the
        // record/name stay alive for this invocation.
        let status =
            unsafe { callback(&mut snapshot.info, core::mem::size_of::<DlPhdrInfo>(), data) };
        if status != 0 {
            return status;
        }
    }
    0
}
