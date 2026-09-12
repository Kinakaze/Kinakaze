//! Named native EAs on a private asynchronous inode handle. No pathname lookup,
//! fallback storage, shared query cursor, or pending request outlives its buffer.

use super::{NativeIoStatus, complete_native_status, object::Object};
use crate::{EIO, ENOSPC, EOPNOTSUPP, errno_from_win32};
use std::{ffi::c_void, ptr};
use windows_sys::Win32::Foundation::HANDLE;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryEaFile(
        file: HANDLE,
        io: *mut NativeIoStatus,
        buffer: *mut c_void,
        length: u32,
        single: u8,
        names: *const c_void,
        names_length: u32,
        index: *const u32,
        restart: u8,
    ) -> i32;
    fn NtSetEaFile(
        file: HANDLE,
        io: *mut NativeIoStatus,
        buffer: *const c_void,
        length: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

fn nt_errno(status: i32) -> i32 {
    match status as u32 {
        0xc000_004f | 0xc000_00bb => EOPNOTSUPP,
        0xc000_0050 => ENOSPC,
        _ => errno_from_win32(unsafe { RtlNtStatusToDosError(status) }),
    }
}

fn validate_name(name: &[u8]) -> Result<(), i32> {
    if name.is_empty() || name.len() > 255 || name.contains(&0) {
        Err(crate::EINVAL)
    } else {
        Ok(())
    }
}

// FILE_GET_EA_INFORMATION starts with a ULONG; NtQueryEaFile probes alignment.
#[repr(align(4))]
struct EaNameBuffer([u8; 6 + 255]);

pub(super) fn read(object: &Object, name: &[u8]) -> Result<Option<Vec<u8>>, i32> {
    validate_name(name)?;
    // The NT EA name is at most 255 bytes. Keep the common absent/small-record
    // query off the managed heap, including path probes during ELF loading.
    let mut name_storage = EaNameBuffer([0u8; 6 + 255]);
    let names = &mut name_storage.0[..6 + name.len()];
    names[4] = name.len() as u8;
    names[5..5 + name.len()].copy_from_slice(name);
    // Ordinary inode records fit on the stack; long symlinks grow
    // only on a native buffer-size status, not an I/O error or timed retry.
    let mut small_storage = [0u64; 64];
    let mut large_storage = Vec::new();
    let mut storage: &mut [u64] = &mut small_storage;
    loop {
        let mut io = NativeIoStatus::default();
        let status = unsafe {
            NtQueryEaFile(
                object.raw(),
                &mut io,
                storage.as_mut_ptr().cast(),
                (storage.len() * 8) as u32,
                1,
                names.as_ptr().cast(),
                names.len() as u32,
                ptr::null(),
                1,
            )
        };
        let status = unsafe { complete_native_status(object.raw(), &mut io, status)? };
        if matches!(status as u32, 0xc000_0051 | 0xc000_0052 | 0x8000_0012) {
            return Ok(None);
        }
        if matches!(status as u32, 0x8000_0005 | 0xc000_0023) && storage.len() < 8192 {
            large_storage.resize(8192, 0);
            storage = &mut large_storage;
            continue;
        }
        if status < 0 {
            return Err(nt_errno(status));
        }
        if status != 0 || io.information < 8 || io.information > storage.len() * 8 {
            return Err(EIO);
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), io.information) };
        let value_len = u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as usize;
        let name_end = 8 + bytes[5] as usize;
        let end = name_end + 1 + value_len;
        if bytes[..4] != [0; 4]
            || end > bytes.len()
            || bytes.get(8..name_end) != Some(name)
            || bytes.get(name_end) != Some(&0)
        {
            return Err(EIO);
        }
        return Ok((value_len != 0).then(|| bytes[name_end + 1..end].to_vec()));
    }
}

pub(super) fn write(object: &Object, name: &[u8], value: &[u8]) -> Result<(), i32> {
    validate_name(name)?;
    let size = 9 + name.len() + value.len();
    if size > 65_535 {
        return Err(ENOSPC);
    }
    let mut storage = vec![0u64; size.div_ceil(8)];
    let bytes = unsafe { std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), size) };
    bytes[5] = name.len() as u8;
    bytes[6..8].copy_from_slice(&(value.len() as u16).to_le_bytes());
    bytes[8..8 + name.len()].copy_from_slice(name);
    bytes[9 + name.len()..].copy_from_slice(value);
    let mut io = NativeIoStatus::default();
    let status = unsafe { NtSetEaFile(object.raw(), &mut io, bytes.as_ptr().cast(), size as u32) };
    let status = unsafe { complete_native_status(object.raw(), &mut io, status)? };
    if status == 0 {
        Ok(())
    } else {
        Err(nt_errno(status))
    }
}
