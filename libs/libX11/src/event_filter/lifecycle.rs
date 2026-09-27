//! Preserve callback order and guest pointers; never copy a native Vec/Mutex.
use super::{Entry, FILTERS, Filter};
use std::{cell::RefCell, sync::MutexGuard};

const KEY: u64 = u64::from_le_bytes(*b"CYXFILT1");
thread_local! {
    static FROZEN: RefCell<Option<MutexGuard<'static, Vec<Entry>>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(filters) = FILTERS.try_lock() else {
            return 11;
        };
        *slot.borrow_mut() = Some(filters);
        0
    })
}

unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(filters) = slot.as_ref() else {
            return -22;
        };
        let size = 16 + filters.len() * 48;
        if !output.is_null() {
            if capacity < size {
                return -22;
            }
            let mut words = vec![KEY, filters.len() as u64];
            for e in filters.iter() {
                words.extend_from_slice(&[
                    e.display as u64,
                    e.window,
                    e.first as i64 as u64,
                    e.last as i64 as u64,
                    e.callback as usize as u64,
                    e.data as u64,
                ]);
            }
            for (index, value) in words.into_iter().enumerate() {
                unsafe {
                    output.add(index * 8).cast::<u64>().write_unaligned(value);
                }
            }
        }
        size as isize
    })
}

unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|slot| slot.borrow_mut().take());
}

unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length < 16 || (length - 16) % 48 != 0 {
        return 22;
    }
    let word = |i: usize| unsafe { input.add(i * 8).cast::<u64>().read_unaligned() };
    let count = (length - 16) / 48;
    if word(0) != KEY || word(1) != count as u64 {
        return 22;
    }
    let mut filters = Vec::new();
    if filters.try_reserve_exact(count).is_err() {
        return 12;
    }
    for i in 0..count {
        let n = 2 + i * 6;
        let Ok(first) = i32::try_from(word(n + 2) as i64) else {
            return 22;
        };
        let Ok(last) = i32::try_from(word(n + 3) as i64) else {
            return 22;
        };
        if word(n + 4) == 0 {
            return 22;
        }
        filters.push(Entry {
            display: word(n) as usize,
            window: word(n + 1),
            first,
            last,
            // Guest ELF addresses and client data are retained by fork's image
            // mappings, just like other registered guest callback addresses.
            callback: unsafe { std::mem::transmute::<usize, Filter>(word(n + 4) as usize) },
            data: word(n + 5) as usize,
        });
    }
    *FILTERS.lock().unwrap_or_else(|e| e.into_inner()) = filters;
    0
}

extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 452,
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
