//! A CUDA context belongs to one native process. A forked copy cannot use it.
//! Fork itself remains available; fresh exec and pre-initialization fork work.

use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static FORKED: AtomicBool = AtomicBool::new(false);

pub(crate) fn initialized() {
    INITIALIZED.store(true, Ordering::Release);
}
pub(crate) fn unusable() -> bool {
    FORKED.load(Ordering::Acquire)
}

unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return 1;
    }
    if capacity < 1 {
        return -22;
    }
    unsafe { output.write(u8::from(INITIALIZED.load(Ordering::Acquire) || unusable())) };
    1
}

unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != 1 {
        return 22;
    }
    let state = unsafe { input.read() };
    if state > 1 {
        return 22;
    }
    FORKED.store(state != 0, Ordering::Release);
    0
}

extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 30,
        key: 0x4355_4441_464f_524b, // CUDAFORK
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(child),
    });
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static REGISTER: extern "C" fn() = register;
