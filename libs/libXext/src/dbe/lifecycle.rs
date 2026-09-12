use super::*;
use core::cell::RefCell;
const KEY: u64 = u64::from_le_bytes(*b"CYXDB001");
thread_local! { static FROZEN: RefCell<Option<buffered::ForkGuard>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        match buffered::freeze() {
            Ok(guard) => {
                *slot.borrow_mut() = Some(guard);
                0
            }
            Err(code) => code,
        }
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
unsafe fn close(display: *mut Display) {
    buffered::close(display as usize);
}
extern "C" fn register() {
    kinakaze_libX11::register_display_close(close);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 446,
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
