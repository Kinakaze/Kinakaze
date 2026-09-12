use super::{CONTROLS, Controls};
use std::{cell::RefCell, sync::MutexGuard};
const KEY: u64 = u64::from_le_bytes(*b"CYXKEY01");
thread_local! {static FROZEN:RefCell<Option<MutexGuard<'static,Controls>>>=const{RefCell::new(None)};}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(s) = CONTROLS.try_lock() else {
            return 11;
        };
        *slot.borrow_mut() = Some(s);
        0
    })
}
unsafe extern "system" fn snapshot(p: *mut u8, n: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(s) = slot.as_ref() else {
            return -22;
        };
        if !p.is_null() {
            if n < 52 {
                return -22;
            }
            let mut b = [0u8; 52];
            b[..8].copy_from_slice(&KEY.to_le_bytes());
            b[8] = s.click;
            b[9] = s.bell;
            b[10..12].copy_from_slice(&s.pitch.to_le_bytes());
            b[12..14].copy_from_slice(&s.duration.to_le_bytes());
            b[14] = s.repeat as u8;
            b[16..20].copy_from_slice(&s.leds.to_le_bytes());
            b[20..52].copy_from_slice(&s.keys);
            unsafe {
                core::ptr::copy_nonoverlapping(b.as_ptr(), p, 52);
            }
        }
        52
    })
}
unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|s| {
        s.borrow_mut().take();
    });
}
unsafe extern "system" fn child(p: *const u8, n: usize) -> i32 {
    if p.is_null() || n != 52 {
        return 22;
    }
    let b = unsafe { core::slice::from_raw_parts(p, n) };
    if b[..8] != KEY.to_le_bytes() || b[8] > 100 || b[9] > 100 || b[14] > 1 {
        return 22;
    }
    *CONTROLS.lock().unwrap() = Controls {
        click: b[8],
        bell: b[9],
        pitch: u16::from_le_bytes(b[10..12].try_into().unwrap()),
        duration: u16::from_le_bytes(b[12..14].try_into().unwrap()),
        repeat: b[14] != 0,
        leds: u32::from_le_bytes(b[16..20].try_into().unwrap()),
        keys: b[20..52].try_into().unwrap(),
    };
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 449,
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
