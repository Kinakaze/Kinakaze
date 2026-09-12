//! Serialize selection records; native locks and raw-input hooks are rebuilt.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const KEY: u64 = u64::from_le_bytes(*b"CYXIF002");
thread_local! {static FROZEN:RefCell<Option<MutexGuard<'static,()>>>=const{RefCell::new(None)};}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(guard) = selection::MUTATION.try_lock() else {
            return 11;
        };
        *slot.borrow_mut() = Some(guard);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    let map = selection::masks().lock().unwrap_or_else(|e| e.into_inner());
    let size = 16 + map.len() * 32;
    if output.is_null() {
        return size as isize;
    }
    if capacity < size {
        return -22;
    }
    let words = [KEY, NEXT_COOKIE.load(Ordering::Acquire) as u64];
    unsafe {
        core::ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), output, 16);
    }
    for (index, ((display, window, device), mask)) in map.iter().enumerate() {
        let words = [*display as u64, *window as u64, *device as u64, *mask];
        unsafe {
            core::ptr::copy_nonoverlapping(
                words.as_ptr().cast::<u8>(),
                output.add(16 + index * 32),
                32,
            );
        }
    }
    size as isize
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, size: usize) -> i32 {
    if input.is_null() || size < 16 || (size - 16) % 32 != 0 {
        return 22;
    }
    let word = |offset| unsafe { input.add(offset).cast::<u64>().read_unaligned() };
    if word(0) != KEY || word(8) > u32::MAX as u64 {
        return 22;
    }
    let mut map = std::collections::BTreeMap::new();
    for offset in (16..size).step_by(32) {
        let (display, window, device, mask) = (
            word(offset),
            word(offset + 8),
            word(offset + 16),
            word(offset + 24),
        );
        if device > 5
            || mask & !0x7fffffe != 0
            || window != 1 && mask & 0x3e000 != 0
            || map
                .insert((display as usize, window as usize, device as i32), mask)
                .is_some()
        {
            return 22;
        }
    }
    *selection::masks().lock().unwrap_or_else(|e| e.into_inner()) = map;
    NEXT_COOKIE.store(word(8) as u32, Ordering::Release);
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
