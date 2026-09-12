//! Only the guest allocation reference crosses fork; native locks are rebuilt.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const KEY: u64 = u64::from_le_bytes(*b"CYPASS01");
thread_local! { static FROZEN: RefCell<Option<MutexGuard<'static, Buffer>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(guard) = BUFFER.try_lock() else {
            return kinakaze_vfs::EAGAIN;
        };
        *slot.borrow_mut() = Some(guard);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, size: usize) -> isize {
    if output.is_null() {
        return 24;
    }
    if size < 24 {
        return -22;
    }
    FROZEN.with(|slot| {
        let guard = slot.borrow();
        let Some(buffer) = guard.as_ref() else {
            return -22;
        };
        let frame = [KEY, buffer.address as u64, buffer.capacity as u64];
        unsafe {
            ptr::copy_nonoverlapping(frame.as_ptr().cast::<u8>(), output, 24);
        }
        24
    })
}
unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, size: usize) -> i32 {
    if input.is_null() || size != 24 {
        return 22;
    }
    let word = |at| unsafe { input.add(at).cast::<u64>().read_unaligned() };
    let address = word(8) as usize;
    let capacity = word(16) as usize;
    if word(0) != KEY
        || (address == 0) != (capacity == 0)
        || (address != 0
            && (!kinakaze_alloc::guest::contains(address)
                || !address
                    .checked_add(capacity - 1)
                    .is_some_and(kinakaze_alloc::guest::contains)))
    {
        return 22;
    }
    let Ok(mut buffer) = BUFFER.lock() else {
        return 5;
    };
    *buffer = Buffer { address, capacity };
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 421,
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
