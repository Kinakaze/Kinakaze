//! Explicit callback addresses are restored after guest mappings; native locks
//! are fresh. No Xlib private heap is copied into the child.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const KEY: u64 = u64::from_le_bytes(*b"CYXER002");
thread_local! {
    static FROZEN: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(guard) = MUTATION.try_lock() else {
            return 11;
        };
        *slot.borrow_mut() = Some(guard);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return 40;
    }
    if capacity < 40 {
        return -22;
    }
    let frame = [
        KEY,
        ERROR.load(Ordering::Acquire) as u64,
        IO_ERROR.load(Ordering::Acquire) as u64,
        IO_EXIT.load(Ordering::Acquire) as u64,
        IO_EXIT_DATA.load(Ordering::Acquire) as u64,
    ];
    unsafe {
        core::ptr::copy_nonoverlapping(frame.as_ptr().cast::<u8>(), output, 40);
    }
    40
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, size: usize) -> i32 {
    if input.is_null() || size != 40 {
        return 22;
    }
    let read = |offset| unsafe { input.add(offset).cast::<u64>().read_unaligned() };
    if read(0) != KEY {
        return 22;
    }
    ERROR.store(read(8) as usize, Ordering::Release);
    IO_ERROR.store(read(16) as usize, Ordering::Release);
    IO_EXIT.store(read(24) as usize, Ordering::Release);
    IO_EXIT_DATA.store(read(32) as usize, Ordering::Release);
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 440,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_frame_requires_exact_size_and_signature() {
        let frame = [0u8; 32];
        for size in [0, 16, 24, 25, 32] {
            assert_eq!(unsafe { child(frame.as_ptr(), size) }, 22);
        }
        assert_eq!(unsafe { child(core::ptr::null(), 24) }, 22);
    }
}
