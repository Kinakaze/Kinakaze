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
struct EaNamesBuffer([u8; 2 * (6 + 255 + 3)]);

pub(super) fn read(object: &Object, name: &[u8]) -> Result<Option<Vec<u8>>, i32> {
    read_private(object.raw(), name)
}

/// Decode bounded inode/verity records directly from the completed native
/// buffer. Only owned decoded fields can escape; no EA allocation or copy is
/// needed for ordinary fixed-size records.
pub(super) fn read_decoded<T>(
    object: &Object,
    name: &[u8],
    decode: impl FnOnce(&[u8]) -> Result<T, i32>,
) -> Result<Option<T>, i32> {
    read_private_decoded(object.raw(), name, decode)
}

/// The caller owns an independent metadata open through completion; borrowing
/// a shared data descriptor here would mix its pending I/O with this query.
pub(crate) fn read_private(handle: HANDLE, name: &[u8]) -> Result<Option<Vec<u8>>, i32> {
    read_private_decoded(handle, name, |bytes| Ok(bytes.to_vec()))
}

fn read_private_decoded<T>(
    handle: HANDLE,
    name: &[u8],
    decode: impl FnOnce(&[u8]) -> Result<T, i32>,
) -> Result<Option<T>, i32> {
    read_values(handle, &[name], false, |values| {
        values[0].map(decode).transpose()
    })
}

/// A named query does not use the file's data offset or EA enumeration cursor.
/// Its completion and cancellation are isolated by the request status block,
/// so a pinned data open with FILE_READ_EA needs no second native open.
pub(crate) fn read_shared_decoded<T>(
    handle: HANDLE,
    name: &[u8],
    decode: impl FnOnce(&[u8]) -> Result<T, i32>,
) -> Result<Option<T>, i32> {
    read_values(handle, &[name], true, |values| {
        values[0].map(decode).transpose()
    })
}

/// Query two related records on one private open. The caller holds the inode
/// transaction when absence or logical EOF must remain stable through a query.
/// Borrowed values exist only while the completed native buffer is live.
pub(super) fn read_pair_decoded<T>(
    object: &Object,
    first: &[u8],
    second: &[u8],
    decode: impl FnOnce(Option<&[u8]>, Option<&[u8]>) -> Result<T, i32>,
) -> Result<T, i32> {
    read_values(object.raw(), &[first, second], false, |values| {
        decode(values[0], values[1])
    })
}

fn read_values<T>(
    handle: HANDLE,
    requested: &[&[u8]],
    shared: bool,
    decode: impl FnOnce([Option<&[u8]>; 2]) -> Result<T, i32>,
) -> Result<T, i32> {
    debug_assert!(!requested.is_empty() && requested.len() <= 2);
    // The NT EA name is at most 255 bytes. Keep the common absent/small-record
    // query off the managed heap, including path probes during ELF loading.
    let mut name_storage = EaNamesBuffer([0u8; 2 * (6 + 255 + 3)]);
    let mut names_len = 0;
    for (index, name) in requested.iter().enumerate() {
        validate_name(name)?;
        if requested[..index].contains(name) {
            return Err(crate::EINVAL);
        }
        let length = 6 + name.len();
        let next = if index + 1 < requested.len() {
            length.next_multiple_of(4)
        } else {
            0
        };
        let record = &mut name_storage.0[names_len..names_len + length];
        record[..4].copy_from_slice(&(next as u32).to_le_bytes());
        record[4] = name.len() as u8;
        record[5..5 + name.len()].copy_from_slice(name);
        names_len += if next == 0 { length } else { next };
    }
    let names = &name_storage.0[..names_len];
    // Ordinary inode records fit on the stack; long symlinks grow
    // only on a native buffer-size status, not an I/O error or timed retry.
    let mut small_storage = [0u64; 64];
    let mut large_storage = Vec::new();
    let mut storage: &mut [u64] = &mut small_storage;
    loop {
        let mut io = NativeIoStatus {
            status: 0x103,
            information: 0,
        };
        let status = unsafe {
            NtQueryEaFile(
                handle,
                &mut io,
                storage.as_mut_ptr().cast(),
                (storage.len() * 8) as u32,
                u8::from(requested.len() == 1),
                names.as_ptr().cast(),
                names.len() as u32,
                ptr::null(),
                1,
            )
        };
        let status = if shared {
            unsafe { super::shared_io::complete(handle, &mut io, status)? }
        } else {
            unsafe { complete_native_status(handle, &mut io, status)? }
        };
        if matches!(status as u32, 0xc000_0051 | 0xc000_0052 | 0x8000_0012) {
            return decode([None; 2]);
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
        let mut values = [None; 2];
        let mut seen = 0u8;
        let mut cursor = 0;
        loop {
            let record = bytes.get(cursor..).filter(|r| r.len() >= 8).ok_or(EIO)?;
            let next = u32::from_le_bytes(record[..4].try_into().unwrap()) as usize;
            let value_len = u16::from_le_bytes(record[6..8].try_into().unwrap()) as usize;
            let name_end = 8 + record[5] as usize;
            let end = name_end + 1 + value_len;
            if end > record.len() || record.get(name_end) != Some(&0) {
                return Err(EIO);
            }
            let index = requested
                .iter()
                .position(|name| record.get(8..name_end) == Some(*name))
                .ok_or(EIO)?;
            if seen & (1 << index) != 0 {
                return Err(EIO);
            }
            seen |= 1 << index;
            if value_len != 0 {
                values[index] = Some(&record[name_end + 1..end]);
            }
            if next == 0 {
                break;
            }
            if !next.is_multiple_of(4) || next < end || next >= record.len() {
                return Err(EIO);
            }
            cursor += next;
        }
        return decode(values);
    }
}

pub(super) fn write(object: &Object, name: &[u8], value: &[u8]) -> Result<(), i32> {
    write_private(object.raw(), name, value)
}

/// Same private-open contract as `read_private`.
pub(crate) fn write_private(handle: HANDLE, name: &[u8], value: &[u8]) -> Result<(), i32> {
    validate_name(name)?;
    let size = 9 + name.len() + value.len();
    if size > 65_535 {
        return Err(ENOSPC);
    }
    let mut small_storage = [0u64; 64];
    let mut large_storage;
    let storage = if size <= std::mem::size_of_val(&small_storage) {
        &mut small_storage[..]
    } else {
        large_storage = vec![0u64; size.div_ceil(8)];
        &mut large_storage[..]
    };
    let bytes = unsafe { std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), size) };
    bytes[5] = name.len() as u8;
    bytes[6..8].copy_from_slice(&(value.len() as u16).to_le_bytes());
    bytes[8..8 + name.len()].copy_from_slice(name);
    bytes[9 + name.len()..].copy_from_slice(value);
    let mut io = NativeIoStatus::default();
    let status = unsafe { NtSetEaFile(handle, &mut io, bytes.as_ptr().cast(), size as u32) };
    let status = unsafe { complete_native_status(handle, &mut io, status)? };
    if status == 0 {
        Ok(())
    } else {
        Err(nt_errno(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA,
    };

    #[test]
    fn compound_query_handles_missing_deleted_and_large_records() {
        let path = std::env::temp_dir().join(format!(
            "kinakaze-ea-pair-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        std::fs::write(&path, b"payload").unwrap();
        let _cleanup = Cleanup(path.clone());
        let object =
            Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA).unwrap();
        let first = b"KINAKAZE.TEST.FIRST";
        let second = b"KINAKAZE.TEST.SECOND";
        let query = |a: &[u8], b: &[u8]| {
            read_pair_decoded(&object, a, b, |a, b| {
                Ok((a.map(<[u8]>::to_vec), b.map(<[u8]>::to_vec)))
            })
            .unwrap()
        };
        assert_eq!(query(first, second), (None, None));
        for size in [1, 320, 4096, 48 * 1024] {
            let data = vec![0x7a; size];
            write(&object, first, &data).unwrap();
            assert_eq!(query(first, second), (Some(data.clone()), None));
            write(&object, second, b"other").unwrap();
            assert_eq!(
                query(first, second),
                (Some(data.clone()), Some(b"other".to_vec()))
            );
            assert_eq!(
                query(second, first),
                (Some(b"other".to_vec()), Some(data.clone()))
            );
            assert_eq!(read(&object, first).unwrap(), Some(data));
            write(&object, second, b"").unwrap();
        }
        write(&object, first, b"").unwrap();
        assert_eq!(query(first, second), (None, None));
    }
}
