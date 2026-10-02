//! Open and reject every native reparse in the same namespace lookup. A
//! preceding attribute query would allow an ancestor-replacement race.
use crate::fs::{NativeIoStatus, complete_native_io, object::Object};
use std::{ffi::c_void, path::Path, ptr};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
};

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *mut UnicodeString,
    attributes: u32,
    security: *mut c_void,
    qos: *mut c_void,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlDosPathNameToNtPathName_U_WithStatus(
        dos: *const u16,
        nt: *mut UnicodeString,
        file: *mut c_void,
        relative: *mut c_void,
    ) -> i32;
    fn RtlFreeUnicodeString(name: *mut UnicodeString);
    fn RtlNtStatusToDosError(status: i32) -> u32;
    fn NtCreateFile(
        file: *mut HANDLE,
        access: u32,
        object: *const ObjectAttributes,
        io: *mut NativeIoStatus,
        allocation: *const i64,
        attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const u8,
        ea_length: u32,
    ) -> i32;
}
struct Name(UnicodeString);
impl Drop for Name {
    fn drop(&mut self) {
        unsafe { RtlFreeUnicodeString(&mut self.0) };
    }
}

pub(super) fn open(path: &Path, access: u32) -> Result<Object, i32> {
    open_with_options(path, access, 0x0020_4000)
}

/// Ordinary file access, without backup privileges or directory semantics.
pub(crate) fn open_file(path: &Path, access: u32) -> Result<Object, i32> {
    open_with_options(
        path,
        access,
        0x0020_0040, // FILE_OPEN_REPARSE_POINT | FILE_NON_DIRECTORY_FILE.
    )
}

/// Pin an ordinary directory while rejecting reparses in every component.
/// The directory constraint also rejects hosted links without an extra query.
pub(crate) fn directory(path: &Path) -> Result<Object, i32> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_TRAVERSE,
    };
    open_with_options(
        path,
        FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_TRAVERSE,
        0x0000_0001, // FILE_DIRECTORY_FILE; OBJ_DONT_REPARSE also rejects the leaf.
    )
}

fn open_with_options(path: &Path, access: u32, options: u32) -> Result<Object, i32> {
    let wide = crate::path::wide_path(path)?;
    let mut name = Name(unsafe { std::mem::zeroed() });
    let status = unsafe {
        RtlDosPathNameToNtPathName_U_WithStatus(
            wide.as_ptr(),
            &mut name.0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status < 0 {
        return Err(crate::errno_from_win32(unsafe {
            RtlNtStatusToDosError(status)
        }));
    }
    let attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root: ptr::null_mut(),
        name: &mut name.0,
        attributes: 0x1000, // OBJ_DONT_REPARSE includes every ancestor.
        security: ptr::null_mut(),
        qos: ptr::null_mut(),
    };
    let mut io = NativeIoStatus {
        status: 0x103,
        information: 0,
    };
    let mut handle = ptr::null_mut();
    let status = unsafe {
        trace_native!(
            "native.NtCreateFile",
            NtCreateFile(
                &mut handle,
                access | SYNCHRONIZE,
                &attributes,
                &mut io,
                ptr::null(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                1, // FILE_OPEN: this speculative lookup never creates an inode.
                options,
                ptr::null(),
                0,
            )
        )
    };
    if status < 0 {
        return Err(crate::errno_from_win32(unsafe {
            RtlNtStatusToDosError(status)
        }));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        // Do not unwind storage still owned by a malformed pending request.
        if status == 0x103 {
            std::process::abort();
        }
        return Err(crate::EIO);
    }
    let object = Object::owned(handle)?;
    // The private open has no unrelated requests. Keep its name/status storage
    // and owned handle alive until even a pending native open has retired.
    unsafe { complete_native_io(handle, &mut io, status)? };
    Ok(object)
}
