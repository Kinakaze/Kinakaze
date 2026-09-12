//! Returned database records contain only guest pointers and guest allocations.
//! A module's calling-thread slot is restored explicitly when that thread forks.
use super::super::records::Names;
use core::{ffi::c_char, ptr};

pub(crate) struct Record(pub usize);
impl Drop for Record {
    fn drop(&mut self) {
        unsafe { kinakaze_alloc::guest::free(self.0 as *mut u8) };
    }
}
impl Record {
    pub(in crate::netdb) fn create(
        names: &Names,
        entry_size: usize,
    ) -> Result<(Self, *mut c_char, *mut *mut c_char), i32> {
        Self::create_extra(names, entry_size, None)
            .map(|(record, name, aliases, _)| (record, name, aliases))
    }
    pub(in crate::netdb) fn create_extra(
        names: &Names,
        entry_size: usize,
        extra: Option<&core::ffi::CStr>,
    ) -> Result<(Self, *mut c_char, *mut *mut c_char, *mut c_char), i32> {
        let count = names.aliases().len() + 1;
        let table = (entry_size + 7) & !7;
        let strings = table
            .checked_add(count.checked_mul(8).ok_or(12)?)
            .ok_or(12)?;
        let all = std::iter::once(names.name.as_c_str())
            .chain(names.aliases().map(|s| s.as_c_str()))
            .chain(extra);
        let size = all
            .clone()
            .try_fold(strings, |n, s| n.checked_add(s.to_bytes_with_nul().len()))
            .ok_or(12)?;
        let base = unsafe { kinakaze_alloc::guest::malloc(size) };
        if base.is_null() {
            return Err(12);
        }
        let aliases = unsafe { base.add(table).cast::<*mut c_char>() };
        let mut offset = strings;
        let mut extra_address = ptr::null_mut();
        for (i, text) in all.enumerate() {
            let target = unsafe { base.add(offset).cast::<c_char>() };
            unsafe {
                ptr::copy_nonoverlapping(text.as_ptr(), target, text.to_bytes_with_nul().len())
            };
            if i != 0 && i < count {
                unsafe { aliases.add(i - 1).write(target) };
            }
            if i == count {
                extra_address = target;
            }
            offset += text.to_bytes_with_nul().len();
        }
        unsafe {
            aliases.add(count - 1).write(ptr::null_mut());
        }
        Ok((
            Self(base as usize),
            unsafe { base.add(strings).cast() },
            aliases,
            extra_address,
        ))
    }
}

macro_rules! returned_record {
    ($magic:expr) => {
        thread_local! { static RETURNED: std::cell::RefCell<Option<super::records::returned::Record>> = const { std::cell::RefCell::new(None) }; }
        fn register_returned() -> Result<(), i32> {
            static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *REGISTERED.get_or_init(|| {
                kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
                    abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
                    priority: 500,
                    key: u64::from_le_bytes($magic),
                    prepare: None,
                    snapshot: Some(snapshot_returned),
                    parent: None,
                    child: Some(restore_returned),
                })
            }) {
                Ok(())
            } else {
                Err(kinakaze_vfs::EIO)
            }
        }
        unsafe extern "system" fn snapshot_returned(output: *mut u8, capacity: usize) -> isize {
            if output.is_null() {
                return 16;
            }
            if capacity < 16 {
                return -22;
            }
            RETURNED.with(|slot| {
                let address = slot.borrow().as_ref().map_or(0, |record| record.0);
                unsafe {
                    core::ptr::copy_nonoverlapping($magic.as_ptr(), output, 8);
                    core::ptr::copy_nonoverlapping(
                        (address as u64).to_le_bytes().as_ptr(),
                        output.add(8),
                        8,
                    );
                }
            });
            16
        }
        unsafe extern "system" fn restore_returned(input: *const u8, length: usize) -> i32 {
            if input.is_null() || length != 16 {
                return 22;
            }
            let bytes = unsafe { core::slice::from_raw_parts(input, length) };
            let address = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
            if bytes[..8] != $magic || (address != 0 && !kinakaze_alloc::guest::contains(address)) {
                return 22;
            }
            RETURNED.with(|slot| {
                *slot.borrow_mut() = (address != 0).then_some(super::records::returned::Record(address))
            });
            0
        }
    };
}
pub(crate) use returned_record;
