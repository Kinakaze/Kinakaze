//! SysV owns restored section views; recreate the native MIT-SHM attachment table.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const KEY: u64 = u64::from_le_bytes(*b"CYXSH002");
thread_local! { static FROZEN: RefCell<Option<MutexGuard<'static, State>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(state) = STATE.try_lock() else {
            return 11;
        };
        if state.active != 0 {
            return 11;
        }
        *slot.borrow_mut() = Some(state);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let state = slot.borrow();
        let Some(state) = state.as_ref() else {
            return -22;
        };
        let size = 24 + state.segments.len() * 32;
        if output.is_null() {
            return size as isize;
        }
        if capacity < size {
            return -22;
        }
        let words = [KEY, state.next, state.segments.len() as u64];
        unsafe {
            ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), output, 24);
        }
        for (index, (id, attachment)) in state.segments.iter().enumerate() {
            let words = [
                *id,
                attachment.address as u64,
                attachment.size as u64,
                u64::from(attachment.read_only),
            ];
            unsafe {
                ptr::copy_nonoverlapping(
                    words.as_ptr().cast::<u8>(),
                    output.add(24 + index * 32),
                    32,
                );
            }
        }
        size as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length < 24 || (length - 24) % 32 != 0 {
        return 22;
    }
    let word = |offset| unsafe { input.add(offset).cast::<u64>().read_unaligned() };
    let next = word(8);
    if word(0) != KEY || next == 0 || word(16) as usize != (length - 24) / 32 {
        return 22;
    }
    let mut segments = BTreeMap::new();
    for offset in (24..length).step_by(32) {
        let (id, address, size, read_only) = (
            word(offset),
            word(offset + 8) as usize,
            word(offset + 16) as usize,
            word(offset + 24),
        );
        if id == 0
            || id >= next
            || read_only > 1
            || segments.contains_key(&id)
            || libc::sysvipc::shm_attachment(address) != Some((size, read_only != 0))
        {
            return 22;
        }
        segments.insert(
            id,
            Arc::new(Attachment {
                display: kinakaze_libX11::shared_display() as usize,
                address,
                size,
                read_only: read_only != 0,
            }),
        );
    }
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = State {
        next,
        active: 0,
        segments,
    };
    0
}

extern "C" fn register() {
    kinakaze_libX11::register_display_close(close_display);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 445,
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
