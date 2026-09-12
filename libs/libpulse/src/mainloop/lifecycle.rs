//! Never copy native Box/Mutex/thread handles into a fork child. Until active
//! Pulse loops have a restorable wire representation, refuse the fork before
//! snapshotting and leave the parent's loop untouched.
use crate::lock;
use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard};

const KEY: u64 = u64::from_le_bytes(*b"CYPML001");
static LIVE: Mutex<usize> = Mutex::new(0);
pub(super) struct Lease;
impl Lease {
    pub(super) fn new() -> Self {
        *lock(&LIVE) += 1;
        Self
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        *lock(&LIVE) -= 1;
    }
}
thread_local! { static FROZEN: RefCell<Option<MutexGuard<'static, usize>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let live = lock(&LIVE);
        if *live != 0 {
            return 11;
        }
        *slot.borrow_mut() = Some(live);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if !FROZEN.with(|slot| slot.borrow().is_some()) {
        return -22;
    }
    if output.is_null() {
        return 8;
    }
    if capacity < 8 {
        return -22;
    }
    unsafe {
        output.cast::<u64>().write_unaligned(KEY);
    }
    8
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if length != 8 || input.is_null() || unsafe { input.cast::<u64>().read_unaligned() } != KEY {
        return 22;
    }
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 450,
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
