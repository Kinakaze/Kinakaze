use super::*;
use std::{cell::RefCell, sync::MutexGuard};
const KEY: u64 = u64::from_le_bytes(*b"CYSYOBJ1");
thread_local! {static FROZEN:RefCell<Option<MutexGuard<'static,Objects>>>=const{RefCell::new(None)};}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|s| {
        if s.borrow().is_some() {
            return 35;
        }
        match OBJECTS.try_lock() {
            Ok(g) => {
                *s.borrow_mut() = Some(g);
                0
            }
            Err(_) => 11,
        }
    })
}
unsafe extern "system" fn snapshot(out: *mut u8, cap: usize) -> isize {
    FROZEN.with(|slot| {
        let guard = slot.borrow();
        let Some(s) = guard.as_ref() else {
            return -22;
        };
        let mut words = vec![
            KEY,
            s.alarms.len() as u64,
            s.fences.len() as u64,
            s.priorities.len() as u64,
        ];
        for (&id, a) in &s.alarms {
            let p = a.attributes;
            words.extend_from_slice(&[
                id as u64,
                a.display as u64,
                p.trigger.counter as u64,
                p.trigger.value_type as u64,
                p.trigger.wait_value.value() as u64,
                p.trigger.test_type as u64,
                p.delta.value() as u64,
                p.events as u64,
                p.state as u64,
                a.previous as u64,
            ]);
        }
        for (&id, &(display, triggered)) in &s.fences {
            words.extend_from_slice(&[id as u64, display as u64, triggered as u64]);
        }
        for (&id, &priority) in &s.priorities {
            words.extend_from_slice(&[id as u64, priority as u64]);
        }
        let bytes = words.len() * 8;
        if out.is_null() {
            return bytes as isize;
        }
        if cap < bytes {
            return -22;
        }
        unsafe {
            ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), out, bytes);
        }
        bytes as isize
    })
}
unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|s| {
        s.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, len: usize) -> i32 {
    if input.is_null() || len < 32 || len % 8 != 0 {
        return 22;
    }
    let words: Vec<u64> = (0..len / 8)
        .map(|i| unsafe { input.add(i * 8).cast::<u64>().read_unaligned() })
        .collect();
    if words[0] != KEY {
        return 22;
    }
    let (a, f, p) = (words[1] as usize, words[2] as usize, words[3] as usize);
    let count = a
        .checked_mul(10)
        .and_then(|v| f.checked_mul(3).and_then(|f| v.checked_add(f)))
        .and_then(|v| p.checked_mul(2).and_then(|p| v.checked_add(p)))
        .and_then(|v| v.checked_add(4));
    if count != Some(words.len()) {
        return 22;
    }
    let mut s = Objects::default();
    let mut n = 4;
    for _ in 0..a {
        let v = &words[n..n + 10];
        n += 10;
        s.alarms.insert(
            v[0] as usize,
            Alarm {
                display: v[1] as usize,
                attributes: AlarmAttributes {
                    trigger: Trigger {
                        counter: v[2] as usize,
                        value_type: v[3] as i32,
                        wait_value: XSyncValue::from_i64(v[4] as i64),
                        test_type: v[5] as i32,
                    },
                    delta: XSyncValue::from_i64(v[6] as i64),
                    events: v[7] as i32,
                    state: v[8] as i32,
                },
                previous: v[9] as i64,
            },
        );
    }
    for _ in 0..f {
        let v = &words[n..n + 3];
        n += 3;
        s.fences.insert(v[0] as usize, (v[1] as usize, v[2] != 0));
    }
    for _ in 0..p {
        let v = &words[n..n + 2];
        n += 2;
        s.priorities.insert(v[0] as usize, v[1] as i32);
    }
    *objects() = s;
    initialize(kinakaze_libX11::shared_display());
    start_timer();
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 448,
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
