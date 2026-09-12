//! Per-database buffered enumeration and fork handoff. Each expansion owns
//! only its own cursor; lookups use separate short-lived readers.
macro_rules! database_cursor {
    ($magic:expr) => {
        use super::super::records::Reader;
        use std::{
            cell::RefCell,
            sync::{Mutex, MutexGuard, OnceLock},
        };
        static CURSOR: Mutex<Option<Reader>> = Mutex::new(None);
        const MAGIC: [u8; 8] = $magic;
        const HEADER: usize = 20;
        thread_local! {
            static PREPARED: RefCell<Option<MutexGuard<'static, Option<Reader>>>> = const { RefCell::new(None) };
        }

        fn register() -> Result<(), i32> {
            static REGISTERED: OnceLock<bool> = OnceLock::new();
            if *REGISTERED.get_or_init(|| {
                kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
                    abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
                    priority: 500,
                    key: u64::from_le_bytes(MAGIC),
                    prepare: Some(prepare),
                    snapshot: Some(snapshot),
                    parent: Some(parent),
                    child: Some(child),
                })
            }) {
                Ok(())
            } else {
                Err(kinakaze_vfs::EIO)
            }
        }

        pub(super) fn rewind() -> Result<(), i32> {
            register()?;
            let mut cursor = CURSOR.lock().map_err(|_| kinakaze_vfs::EIO)?;
            match cursor.as_mut() {
                Some(reader) => reader.rewind(),
                None => {
                    *cursor = Some(Reader::open(super::PATH)?);
                    Ok(())
                }
            }
        }
        pub(super) fn close() -> Result<(), i32> {
            CURSOR.lock().map_err(|_| kinakaze_vfs::EIO)?.take();
            Ok(())
        }
        pub(super) fn next<T>(read: impl FnOnce(&mut Reader) -> Result<T, i32>) -> Result<T, i32> {
            register()?;
            let mut cursor = CURSOR.lock().map_err(|_| kinakaze_vfs::EIO)?;
            if cursor.is_none() {
                *cursor = Some(Reader::open(super::PATH)?);
            }
            read(cursor.as_mut().unwrap())
        }

        unsafe extern "system" fn prepare() -> i32 {
            PREPARED.with(|slot| {
                let mut slot = slot.borrow_mut();
                if slot.is_some() {
                    return 35; // EDEADLK: nested handoff on the preparing thread.
                }
                match CURSOR.lock() {
                    Ok(guard) => {
                        *slot = Some(guard);
                        0
                    }
                    Err(_) => kinakaze_vfs::EIO,
                }
            })
        }
        unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
            PREPARED.with(|slot| {
                let guard = slot.borrow();
                let Some(cursor) = guard.as_ref() else {
                    return -(kinakaze_vfs::EINVAL as isize);
                };
                let reader = cursor.as_ref();
                let bytes = reader.map_or(&[][..], Reader::unread);
                let length = HEADER + bytes.len();
                if output.is_null() {
                    return length as isize;
                }
                if capacity < length {
                    return -(kinakaze_vfs::EINVAL as isize);
                }
                let output = unsafe { std::slice::from_raw_parts_mut(output, length) };
                output[..8].copy_from_slice(&MAGIC);
                output[8..12].copy_from_slice(&reader.map_or(-1, Reader::fd).to_le_bytes());
                output[12..16].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
                output[16..20].copy_from_slice(&u32::from(reader.is_some_and(Reader::eof)).to_le_bytes());
                output[HEADER..].copy_from_slice(bytes);
                length as isize
            })
        }
        unsafe extern "system" fn parent(_status: i32) {
            PREPARED.with(|slot| {
                slot.borrow_mut().take();
            });
        }
        unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
            if input.is_null() || !(HEADER..=HEADER + Reader::CAPACITY).contains(&length) {
                return kinakaze_vfs::EINVAL;
            }
            let bytes = unsafe { std::slice::from_raw_parts(input, length) };
            let fd = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
            let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let eof = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
            if bytes[..8] != MAGIC
                || count != length - HEADER
                || fd < -1
                || eof > 1
                || (fd == -1 && (count != 0 || eof != 0))
            {
                return kinakaze_vfs::EINVAL;
            }
            let reader = if fd == -1 {
                None
            } else {
                // VFS restores descriptors at priority 50, before this callback.
                if let Err(error) = kinakaze_vfs::get(fd) {
                    return error;
                }
                Some(Reader::from_parts(fd, &bytes[HEADER..], eof != 0))
            };
            match CURSOR.lock() {
                Ok(mut cursor) => {
                    *cursor = reader;
                    0
                }
                Err(_) => kinakaze_vfs::EIO,
            }
        }
    };
}
pub(crate) use database_cursor;
