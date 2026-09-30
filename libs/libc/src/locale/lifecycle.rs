//! Only the global category bits, immutable guest data pointer and calling
//! thread's locale selection cross fork. Mutexes and native TLS stay local.
use super::*;
use core::cell::RefCell;
use std::sync::MutexGuard;
const FRAME: usize = 48;
const KEY: u64 = u64::from_le_bytes(*b"CYLOCF02");
thread_local! {
    static FROZEN: RefCell<Option<MutexGuard<'static,()>>> = const { RefCell::new(None) };
}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(guard) = MUTATION.try_lock() else {
            return kinakaze_vfs::EAGAIN;
        };
        *slot.borrow_mut() = Some(guard);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return FRAME as isize;
    }
    if capacity < FRAME {
        return -(EINVAL as isize);
    }
    let fields = [
        KEY,
        GLOBAL_FLAGS.load(Ordering::Acquire) as u64,
        UTF8_DATA.load(Ordering::Acquire) as u64,
        kinakaze_tls::locale() as u64,
        unsafe { kinakaze_abi__nl_msg_cat_cntr.get() } as u32 as u64,
        kinakaze_abi__nl_msg_cat_cntr.target() as u64,
    ];
    for (index, word) in fields.into_iter().enumerate() {
        unsafe {
            ptr::copy_nonoverlapping(word.to_le_bytes().as_ptr(), output.add(index * 8), 8);
        }
    }
    FRAME as isize
}
unsafe extern "system" fn parent(_result: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != FRAME {
        return EINVAL;
    }
    let bytes = unsafe { core::slice::from_raw_parts(input, length) };
    let fields: [u64; 6] =
        core::array::from_fn(|i| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap()));
    if fields[0] != KEY
        || fields[1] & !(MASK as u64) != 0
        || !valid(fields[3] as usize)
        || (fields[2] != 0 && !guest::contains(fields[2] as usize))
        || fields[4] > u32::MAX as u64
        // COPY targets live in ELF image mappings, outside the guest heap.
        || fields[5] % core::mem::align_of::<c_int>() as u64 != 0
    {
        return EINVAL;
    }
    if fields[2] != 0 {
        let address = fields[2] as usize;
        if address % core::mem::align_of::<usize>() != 0
            || !address
                .checked_add(core::mem::size_of::<usize>() - 1)
                .is_some_and(guest::contains)
        {
            return EINVAL;
        }
        let size = unsafe { *(address as *const usize) };
        if size < 8
            || size > 16 * 1024 * 1024
            || !address
                .checked_add(core::mem::size_of::<usize>())
                .and_then(|p| p.checked_add(size - 1))
                .is_some_and(guest::contains)
        {
            return EINVAL;
        }
    }
    let thread_flags = if fields[3] as usize == GLOBAL {
        fields[1] as u32
    } else {
        flags(fields[3] as usize)
    };
    if (thread_flags | fields[1] as u32) & 1 != 0 && fields[2] == 0 {
        return EINVAL;
    }
    GLOBAL_FLAGS.store(fields[1] as u32, Ordering::Release);
    UTF8_DATA.store(fields[2] as usize, Ordering::Release);
    kinakaze_tls::set_locale(fields[3] as usize);
    unsafe {
        kinakaze_abi__nl_msg_cat_cntr.redirect(fields[5] as *mut c_int);
        kinakaze_abi__nl_msg_cat_cntr.set(fields[4] as c_int);
    }
    0
}
extern "C" fn register() {
    let _ = unsafe { kinakaze_runtime::register_fork_participant_without_inherited_handles(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 400,
        key: KEY,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    }) };
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
