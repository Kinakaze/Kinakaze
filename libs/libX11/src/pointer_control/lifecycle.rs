//! Only the saved defaults are private process state. Current desktop settings
//! belong to Windows and must never be replayed when restoring a fork child.
use super::DEFAULTS;
use core::cell::RefCell;
use std::sync::MutexGuard;
const MAGIC: u64 = u64::from_le_bytes(*b"CYXPTR01");
thread_local! { static FROZEN: RefCell<Option<MutexGuard<'static, Option<[i32; 3]>>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        let Ok(guard) = DEFAULTS.try_lock() else {
            return 11;
        };
        *slot = Some(guard);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(guard) = slot.as_ref() else {
            return -22;
        };
        if !output.is_null() {
            if capacity < 32 {
                return -22;
            }
            let mut bytes = [0_u8; 32];
            bytes[..8].copy_from_slice(&MAGIC.to_le_bytes());
            bytes[8] = guard.is_some() as u8;
            if let Some(values) = **guard {
                for (i, value) in values.iter().enumerate() {
                    bytes[16 + i * 4..20 + i * 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), output, 32);
            }
        }
        32
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != 32 {
        return 22;
    }
    let bytes = unsafe { core::slice::from_raw_parts(input, length) };
    if u64::from_le_bytes(bytes[..8].try_into().unwrap()) != MAGIC
        || bytes[8] > 1
        || bytes[9..16].iter().chain(&bytes[28..]).any(|b| *b != 0)
    {
        return 22;
    }
    let value = if bytes[8] == 0 {
        if bytes[16..28].iter().any(|b| *b != 0) {
            return 22;
        }
        None
    } else {
        let values = core::array::from_fn(|i| {
            i32::from_le_bytes(bytes[16 + i * 4..20 + i * 4].try_into().unwrap())
        });
        if values[0] < 0 || values[1] < 0 || !(0..=2).contains(&values[2]) {
            return 22;
        }
        Some(values)
    };
    *DEFAULTS.lock().unwrap() = value;
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
