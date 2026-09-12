//! GNU loader information derived from actual ELF objects and native images.

use super::{clear_dl_error, dynamic_loader_lock, loaded_linker, set_dl_error};
use core::ffi::{c_char, c_void};
use std::ffi::CString;
use std::ptr;

#[repr(C)]
struct DlSerInfo {
    size: usize,
    count: u32,
    padding: u32,
}

#[repr(C)]
struct DlSerPath {
    name: *mut c_char,
    flags: u32,
    padding: u32,
}

fn search_size(paths: &[CString]) -> Option<usize> {
    let records = paths.len().checked_mul(size_of::<DlSerPath>())?;
    let start = size_of::<DlSerInfo>().checked_add(records)?;
    paths.iter().try_fold(start, |size, path| {
        size.checked_add(path.as_bytes_with_nul().len())
    })
}

unsafe fn write_search_info(
    paths: &[CString],
    size_only: bool,
    argument: *mut c_void,
) -> Result<(), &'static str> {
    let required = search_size(paths).ok_or("dynamic loader search path size overflow")?;
    let count = u32::try_from(paths.len()).map_err(|_| "too many dynamic loader search paths")?;
    let header = argument.cast::<DlSerInfo>();
    if size_only {
        unsafe {
            ptr::write_unaligned(
                header,
                DlSerInfo {
                    size: required,
                    count,
                    padding: 0,
                },
            )
        };
        return Ok(());
    }
    let supplied_size = unsafe { ptr::read_unaligned(&raw const (*header).size) };
    let supplied_count = unsafe { ptr::read_unaligned(&raw const (*header).count) };
    if supplied_size < required || supplied_count < count {
        return Err("dlinfo SERINFO buffer is smaller than SERINFOSIZE requires");
    }
    let records = unsafe {
        argument
            .cast::<u8>()
            .add(size_of::<DlSerInfo>())
            .cast::<DlSerPath>()
    };
    let mut name = unsafe {
        argument
            .cast::<u8>()
            .add(size_of::<DlSerInfo>() + paths.len() * size_of::<DlSerPath>())
    };
    for (index, path) in paths.iter().enumerate() {
        let bytes = path.as_bytes_with_nul();
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), name, bytes.len());
            // GNU currently leaves this reserved source-flags field zero.
            ptr::write_unaligned(
                records.add(index),
                DlSerPath {
                    name: name.cast(),
                    flags: 0,
                    padding: 0,
                },
            );
            name = name.add(bytes.len());
        }
    }
    unsafe {
        ptr::write_unaligned(
            header,
            DlSerInfo {
                size: required,
                count,
                padding: 0,
            },
        )
    };
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_dlinfo(
    handle: *mut c_void,
    request: i32,
    argument: *mut c_void,
) -> i32 {
    if handle.is_null() || argument.is_null() {
        set_dl_error("dlinfo requires a live handle and writable result");
        return -1;
    }
    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if linker.is_null() {
        set_dl_error("no active process dynamic linker");
        return -1;
    }
    let info = match unsafe { (&mut *linker).runtime_object_info(handle as usize) } {
        Ok(info) => info,
        Err(error) => {
            set_dl_error(error);
            return -1;
        }
    };
    let result = match request {
        1 => {
            // RTLD_DI_LMID: one actual base namespace is currently loaded.
            unsafe { ptr::write_unaligned(argument.cast::<i64>(), 0) };
            0
        }
        2 => {
            // RTLD_DI_LINKMAP
            unsafe {
                ptr::write_unaligned(
                    argument.cast::<*const crate::linker::GuestLinkMap>(),
                    &raw const *info.map,
                )
            };
            0
        }
        4 | 5 => {
            if let Err(error) =
                unsafe { write_search_info(&info.search_paths, request == 5, argument) }
            {
                set_dl_error(error);
                return -1;
            }
            0
        }
        6 => {
            // RTLD_DI_ORIGIN: the ABI requires caller-provided ample space.
            let bytes = info.origin.as_bytes_with_nul();
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), argument.cast::<u8>(), bytes.len()) };
            0
        }
        9 => {
            unsafe { ptr::write_unaligned(argument.cast::<usize>(), info.tls_module.unwrap_or(0)) };
            0
        }
        10 => {
            let data = info
                .tls_module
                .and_then(kinakaze_tls::current_elf_tls_data)
                .unwrap_or(ptr::null_mut());
            unsafe { ptr::write_unaligned(argument.cast::<*mut u8>(), data) };
            0
        }
        11 => {
            unsafe { ptr::write_unaligned(argument.cast::<usize>(), info.program_headers) };
            i32::from(info.program_count)
        }
        _ => {
            set_dl_error(format_args!("unsupported dlinfo request {request}"));
            return -1;
        }
    };
    clear_dl_error();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_paths_fill_only_the_sized_buffer_with_in_buffer_names() {
        let paths = [c"/usr/lib".to_owned(), c"/opt/java/lib".to_owned()];
        let mut header = DlSerInfo {
            size: 0,
            count: 0,
            padding: 0,
        };
        unsafe { write_search_info(&paths, true, (&raw mut header).cast()) }.unwrap();
        assert_eq!(header.count, 2);
        let mut bytes = vec![0xa5u8; header.size + 16];
        unsafe { ptr::write_unaligned(bytes.as_mut_ptr().cast(), header) };
        unsafe { write_search_info(&paths, false, bytes.as_mut_ptr().cast()) }.unwrap();
        let records = unsafe { bytes.as_ptr().add(16).cast::<DlSerPath>() };
        for (index, path) in paths.iter().enumerate() {
            let record = unsafe { ptr::read_unaligned(records.add(index)) };
            assert_eq!(
                unsafe { std::ffi::CStr::from_ptr(record.name) },
                path.as_c_str()
            );
            assert!(record.name as usize >= bytes.as_ptr() as usize + 48);
            assert_eq!(record.flags, 0);
        }
        assert!(
            bytes[search_size(&paths).unwrap()..]
                .iter()
                .all(|byte| *byte == 0xa5)
        );
    }

    #[test]
    fn undersized_search_buffer_is_rejected_before_writing() {
        let paths = [c"/usr/lib".to_owned()];
        let mut bytes = vec![0xa5u8; 128];
        unsafe {
            ptr::write_unaligned(
                bytes.as_mut_ptr().cast(),
                DlSerInfo {
                    size: 16,
                    count: 1,
                    padding: 0,
                },
            )
        };
        let original = bytes.clone();
        assert!(unsafe { write_search_info(&paths, false, bytes.as_mut_ptr().cast()) }.is_err());
        assert_eq!(bytes, original);
    }
}
