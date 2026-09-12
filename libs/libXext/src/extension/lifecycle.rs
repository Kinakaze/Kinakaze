//! Restore only the root of guest-owned lists; all native mutexes start fresh.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const KEY: u64 = u64::from_le_bytes(*b"CYXEX001");
thread_local! { static FROZEN: RefCell<Option<MutexGuard<'static, usize>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(guard) = LISTS.try_lock() else {
            return 11;
        };
        *slot.borrow_mut() = Some(guard);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let guard = slot.borrow();
        let Some(guard) = guard.as_ref() else {
            return -22;
        };
        if output.is_null() {
            return 16;
        }
        if capacity < 16 {
            return -22;
        }
        let words = [KEY, **guard as u64];
        unsafe {
            ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), output, 16);
        }
        16
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != 16 {
        return 22;
    }
    if unsafe { input.cast::<u64>().read_unaligned() } != KEY {
        return 22;
    }
    let address = unsafe { input.add(8).cast::<u64>().read_unaligned() } as usize;
    if address != 0
        && (address % mem::align_of::<OwnedInfo>() != 0
            || !guest::contains(address)
            || !address
                .checked_add(mem::size_of::<OwnedInfo>() - 1)
                .is_some_and(guest::contains))
    {
        return 22;
    }
    *LISTS.lock().unwrap_or_else(|e| e.into_inner()) = address;
    0
}
extern "C" fn register() {
    kinakaze_libX11::register_display_close(close_display);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 444,
        key: KEY,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
