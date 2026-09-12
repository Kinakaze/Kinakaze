//! Transfer fixed TrueColor map identifiers and installation state only.
use super::{STATE, State};
use core::cell::RefCell;
use std::sync::MutexGuard;
const MAGIC: u64 = u64::from_le_bytes(*b"CYXCLR01");
struct Frozen {
    _state: MutexGuard<'static, State>,
    bytes: Vec<u8>,
}
thread_local! { static FROZEN:RefCell<Option<Frozen>>=const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        let Ok(state) = STATE.try_lock() else {
            return 11;
        };
        let mut bytes = Vec::with_capacity(24 + state.maps.len() * 8);
        for word in [MAGIC, state.maps.len() as u64, state.installed as u64]
            .into_iter()
            .chain(state.maps.iter().map(|v| *v as u64))
        {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        *slot = Some(Frozen {
            _state: state,
            bytes,
        });
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(frozen) = slot.as_ref() else {
            return -22;
        };
        if !output.is_null() {
            if capacity < frozen.bytes.len() {
                return -22;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(frozen.bytes.as_ptr(), output, frozen.bytes.len());
            }
        }
        frozen.bytes.len() as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length < 24 || length > isize::MAX as usize || length % 8 != 0 {
        return 22;
    }
    let bytes = unsafe { core::slice::from_raw_parts(input, length) };
    let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
    if word(0) != MAGIC as usize || word(8) != (length - 24) / 8 {
        return 22;
    }
    let mut state = State {
        maps: Default::default(),
        installed: word(16),
    };
    for offset in (24..length).step_by(8) {
        let map = word(offset);
        if map <= 1 || map >= u32::MAX as usize || !state.maps.insert(map) {
            return 22;
        }
    }
    if state.installed != 1 && !state.maps.contains(&state.installed) {
        return 22;
    }
    if let Some(last) = state.maps.last() {
        crate::graphics::reserve_drawable_ids(*last + 1);
    }
    *STATE.lock().unwrap() = state;
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 449,
        key: MAGIC,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
