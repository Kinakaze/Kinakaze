use super::{FOCUS, Focus, Grab};
use std::{cell::RefCell, sync::MutexGuard};
const KEY: u64 = u64::from_le_bytes(*b"CYXFOC02");
thread_local! {static FROZEN:RefCell<Option<MutexGuard<'static,Focus>>>=const{RefCell::new(None)};}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(s) = FOCUS.try_lock() else {
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
            if n < 56 {
                return -22;
            }
            let b = [
                KEY,
                s.mode as u64 | ((s.revert as u64) << 8),
                s.window,
                s.time as u64,
                s.grab.map_or(0, |g| g.window),
                s.grab.map_or(0, |g| g.time as u64),
                s.grab.map_or(0, |g| g.owner_events as u64),
            ];
            for (i, v) in b.into_iter().enumerate() {
                unsafe { p.add(i * 8).cast::<u64>().write_unaligned(v) };
            }
        }
        56
    })
}
unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|s| {
        s.borrow_mut().take();
    });
}
unsafe extern "system" fn child(p: *const u8, n: usize) -> i32 {
    if p.is_null() || n != 56 {
        return 22;
    }
    let word = |i: usize| unsafe { p.add(i * 8).cast::<u64>().read_unaligned() };
    let modes = word(1);
    let mode = modes as u8;
    let revert = (modes >> 8) as u8;
    if word(0) != KEY || modes > 0x202 || mode > 2 || revert > 2 || word(3) > u32::MAX as u64 {
        return 22;
    }
    if word(5) > u32::MAX as u64 || word(6) > 1 {
        return 22;
    }
    let grab = if word(4) == 0 {
        None
    } else {
        if super::native(word(4)).is_none() {
            return 22;
        }
        Some(Grab {
            window: word(4),
            time: word(5) as u32,
            owner_events: word(6) != 0,
        })
    };
    if mode == 2 {
        let Some(window) = super::native(word(2)) else {
            return 22;
        };
        if !kinakaze_libdisplay::ui::set_focus(window) {
            return 5;
        }
    }
    *FOCUS.lock().unwrap() = Focus {
        mode,
        window: word(2),
        revert,
        time: word(3) as u32,
        grab,
    };
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 451,
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
