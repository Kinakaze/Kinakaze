use super::*;
use std::{cell::RefCell, sync::MutexGuard};
const KEY: u64 = u64::from_le_bytes(*b"CYXSY001");
thread_local! { static FROZEN:RefCell<Option<MutexGuard<'static,Option<Counters>>>>=const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|s| {
        if s.borrow().is_some() {
            return 35;
        }
        *s.borrow_mut() = Some(COUNTERS.lock().unwrap_or_else(|e| e.into_inner()));
        0
    })
}
unsafe extern "system" fn snapshot(out: *mut u8, cap: usize) -> isize {
    FROZEN.with(|slot| {
        let frozen = slot.borrow();
        let Some(g) = frozen.as_ref() else {
            return -22;
        };
        let s = g.as_ref();
        let count = s.map_or(0, |s| s.values.len());
        let len = 24 + count * 24;
        if out.is_null() {
            return len as isize;
        }
        if cap < len {
            return -22;
        }
        unsafe {
            out.cast::<u64>().write_unaligned(KEY);
            out.add(8)
                .cast::<u64>()
                .write_unaligned(s.map_or(0x20_0000, |s| s.next) as u64);
            out.add(16).cast::<u64>().write_unaligned(count as u64);
            if let Some(s) = s {
                for (i, (id, c)) in s.values.iter().enumerate() {
                    let p = out.add(24 + i * 24);
                    p.cast::<u64>().write_unaligned(*id as u64);
                    p.add(8).cast::<u64>().write_unaligned(c.display as u64);
                    p.add(16).cast::<i64>().write_unaligned(c.value);
                }
            }
        }
        len as isize
    })
}
unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|s| {
        s.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, len: usize) -> i32 {
    if input.is_null() || len < 24 {
        return 22;
    }
    unsafe {
        if input.cast::<u64>().read_unaligned() != KEY {
            return 22;
        }
        let count = input.add(16).cast::<u64>().read_unaligned() as usize;
        if count.checked_mul(24).and_then(|n| n.checked_add(24)) != Some(len) {
            return 22;
        }
        let mut s = Counters {
            next: input.add(8).cast::<u64>().read_unaligned() as usize,
            values: HashMap::new(),
        };
        for i in 0..count {
            let p = input.add(24 + i * 24);
            s.values.insert(
                p.cast::<u64>().read_unaligned() as usize,
                Counter {
                    display: p.add(8).cast::<u64>().read_unaligned() as usize,
                    value: p.add(16).cast::<i64>().read_unaligned(),
                },
            );
        }
        *COUNTERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    }
    0
}
extern "C" fn register() {
    kinakaze_libX11::register_display_close(close);
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
