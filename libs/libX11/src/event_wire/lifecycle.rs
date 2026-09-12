//! Serialize callback entries, not the Rust map or its synchronization state.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const KEY: u64 = u64::from_le_bytes(*b"CYXEW001");
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
fn address_to_frame(address: usize) -> usize {
    if address == unknown as *const () as usize {
        0
    } else if address == _XWireToEvent as *const () as usize {
        1
    } else if address == _XEventToWire as *const () as usize {
        2
    } else {
        address
    }
}
fn address_from_frame(value: usize) -> usize {
    match value {
        0 => unknown as *const () as usize,
        1 => _XWireToEvent as *const () as usize,
        2 => _XEventToWire as *const () as usize,
        _ => value,
    }
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let state = slot.borrow();
        let Some(state) = state.as_ref() else {
            return -22;
        };
        let size = 16 + state.callbacks.len() * 32;
        if output.is_null() {
            return size as isize;
        }
        if capacity < size {
            return -22;
        }
        let put = |offset, value: usize| unsafe {
            output
                .add(offset)
                .cast::<u64>()
                .write_unaligned(value as u64);
        };
        put(0, KEY as usize);
        put(8, state.callbacks.len());
        for (index, (&(display, kind, outgoing), &address)) in state.callbacks.iter().enumerate() {
            let offset = 16 + index * 32;
            put(
                offset,
                if display == (&raw mut crate::GLOBAL_DISPLAY) as usize {
                    1
                } else {
                    display
                },
            );
            put(offset + 8, kind as usize);
            put(offset + 16, usize::from(outgoing));
            put(offset + 24, address_to_frame(address));
        }
        size as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, size: usize) -> i32 {
    if input.is_null() || size < 16 {
        return 22;
    }
    let get = |offset| unsafe { input.add(offset).cast::<u64>().read_unaligned() as usize };
    let count = get(8);
    if get(0) != KEY as usize || count.checked_mul(32).and_then(|n| n.checked_add(16)) != Some(size)
    {
        return 22;
    }
    let mut callbacks = BTreeMap::new();
    for index in 0..count {
        let offset = 16 + index * 32;
        let display = get(offset);
        let kind = get(offset + 8);
        let outgoing = get(offset + 16);
        if kind > 127 || outgoing > 1 {
            return 22;
        }
        let display = if display == 1 {
            (&raw mut crate::GLOBAL_DISPLAY) as usize
        } else {
            display
        };
        if callbacks
            .insert(
                (display, kind as u8, outgoing != 0),
                address_from_frame(get(offset + 24)),
            )
            .is_some()
        {
            return 22;
        }
    }
    STATE.lock().unwrap_or_else(|e| e.into_inner()).callbacks = callbacks;
    0
}
unsafe fn close(display: *mut Display) {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .callbacks
        .retain(|&(owner, _, _), _| owner != display as usize);
}
extern "C" fn register() {
    crate::register_display_close(close);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 447,
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
    fn native_defaults_are_rebound_and_frames_are_bounded() {
        for address in [
            unknown as *const () as usize,
            _XWireToEvent as *const () as usize,
            _XEventToWire as *const () as usize,
            0x12340000,
        ] {
            assert_eq!(address_from_frame(address_to_frame(address)), address);
        }
        let mut frame = [KEY, 1, 0, 128, 0, 0];
        assert_eq!(unsafe { child(frame.as_ptr().cast(), 48) }, 22);
        frame[3] = 33;
        frame[4] = 2;
        assert_eq!(unsafe { child(frame.as_ptr().cast(), 48) }, 22);
        frame[1] = u64::MAX;
        assert_eq!(unsafe { child(frame.as_ptr().cast(), 48) }, 22);
        assert_eq!(unsafe { child(core::ptr::null(), 0) }, 22);
    }
}
