//! A separate, lazily committed program-break arena. The native malloc arena
//! remains independent, so legacy MORECORE users cannot corrupt its metadata.
use super::*;
use std::sync::{Mutex, OnceLock};
const CAPACITY: usize = 4 * 1024 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"CYBRK001";
#[derive(Clone, Copy, Default)]
struct Break {
    base: usize,
    current: usize,
    committed: usize,
}
static STATE: Mutex<Break> = Mutex::new(Break {
    base: 0,
    current: 0,
    committed: 0,
});
#[unsafe(no_mangle)]
pub static kinakaze_abi___curbrk: crate::copied::CopiedValue<usize> =
    crate::copied::CopiedValue::new(0);

fn register() -> Result<(), i32> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    if *REGISTERED.get_or_init(|| {
        kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
            abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
            priority: 500,
            key: u64::from_le_bytes(*MAGIC),
            prepare: None,
            snapshot: Some(snapshot),
            parent: None,
            child: Some(restore),
        })
    }) {
        Ok(())
    } else {
        Err(5)
    }
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return 32;
    }
    if capacity < 32 {
        return -22;
    }
    let Ok(state) = STATE.lock() else {
        return -5;
    };
    unsafe {
        ptr::copy_nonoverlapping(MAGIC.as_ptr(), output, 8);
        for (index, value) in [state.base, state.current, state.committed]
            .into_iter()
            .enumerate()
        {
            output
                .add(8 + index * 8)
                .cast::<usize>()
                .write_unaligned(value);
        }
    }
    32
}
unsafe extern "system" fn restore(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != 32 || unsafe { core::slice::from_raw_parts(input, 8) } != MAGIC
    {
        return 22;
    }
    let read = |offset| unsafe { input.add(offset).cast::<usize>().read_unaligned() };
    let state = Break {
        base: read(8),
        current: read(16),
        committed: read(24),
    };
    if state.current < state.base
        || state.current - state.base > CAPACITY
        || state.committed > CAPACITY
        || state.committed % 4096 != 0
    {
        return 22;
    }
    let Ok(mut slot) = STATE.lock() else {
        return 5;
    };
    *slot = state;
    unsafe {
        kinakaze_abi___curbrk.set(state.current);
    }
    0
}
fn initialize(state: &mut Break) -> Result<(), i32> {
    if state.base != 0 {
        return Ok(());
    }
    register()?;
    let pointer = unsafe { kinakaze_abi_mmap(ptr::null_mut(), CAPACITY, 0, 0x22, -1, 0) };
    if pointer == MAP_FAILED {
        return Err(kinakaze_tls::errno());
    }
    state.base = pointer as usize;
    state.current = state.base;
    unsafe {
        kinakaze_abi___curbrk.set(state.current);
    }
    Ok(())
}
fn resize(state: &mut Break, address: usize) -> Result<(), i32> {
    if address < state.base || address - state.base > CAPACITY {
        return Err(ENOMEM);
    }
    let wanted = (address - state.base + 4095) & !4095;
    if wanted > state.committed {
        if unsafe {
            kinakaze_abi_mprotect(
                (state.base + state.committed) as *mut c_void,
                wanted - state.committed,
                3,
            )
        } < 0
        {
            return Err(kinakaze_tls::errno());
        }
    } else if wanted < state.committed {
        let start = (state.base + wanted) as *mut c_void;
        let size = state.committed - wanted;
        if unsafe { kinakaze_abi_madvise(start, size, 4) } < 0
            || unsafe { kinakaze_abi_mprotect(start, size, 0) } < 0
        {
            return Err(kinakaze_tls::errno());
        }
    }
    state.current = address;
    state.committed = wanted;
    unsafe {
        kinakaze_abi___curbrk.set(address);
    }
    Ok(())
}
pub(super) fn brk(address: usize) -> Result<(), i32> {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(12)?;
    let mut state = STATE.lock().map_err(|_| 5)?;
    initialize(&mut state)?;
    if address != 0 {
        resize(&mut state, address)?;
    }
    Ok(())
}
pub(super) fn sbrk(increment: isize) -> Result<usize, i32> {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(12)?;
    let mut state = STATE.lock().map_err(|_| 5)?;
    initialize(&mut state)?;
    let old = state.current;
    let next = old.checked_add_signed(increment).ok_or(12)?;
    resize(&mut state, next)?;
    Ok(old)
}
