//! Retained section slots restore SysV views at their original guest addresses.
use super::*;
use core::cell::RefCell;
use std::sync::{
    MutexGuard,
    atomic::{AtomicUsize, Ordering},
};
const KEY: u64 = u64::from_le_bytes(*b"CYSHMF01");
struct Frozen {
    _state: MutexGuard<'static, IpcState>,
    bytes: Vec<u8>,
}
thread_local! { static FROZEN: RefCell<Option<Frozen>> = const { RefCell::new(None) }; }
fn word(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(state) = state().try_lock() else {
            return EAGAIN;
        };
        // Semaphore undo/waiter state has a separate lifecycle contract.
        if !state.sets.is_empty() {
            return EAGAIN;
        }
        let mut segments: std::collections::BTreeMap<_, _> = state
            .segments
            .iter()
            .map(|(id, s)| (*id, s.as_ref()))
            .collect();
        for attachment in state.attachments.values() {
            segments.insert(attachment.id, attachment.segment.as_ref());
        }
        let mut bytes = Vec::new();
        for value in [
            KEY,
            state.next_id as u64,
            segments.len() as u64,
            state.attachments.len() as u64,
            state.removed.len() as u64,
        ] {
            word(&mut bytes, value);
        }
        for (id, segment) in segments {
            for value in [
                id as u64,
                segment.namespace,
                segment.key as u32 as u64,
                segment.segsz as u64,
                segment.fork_slot as u64,
                segment.private_lock,
                u64::from(state.segments.contains_key(&id)),
            ] {
                word(&mut bytes, value);
            }
        }
        for (address, attachment) in &state.attachments {
            for value in [
                *address as u64,
                attachment.id as u64,
                u64::from(attachment.read_only),
            ] {
                word(&mut bytes, value);
            }
        }
        for id in &state.removed {
            word(&mut bytes, *id as u64);
        }
        *slot.borrow_mut() = Some(Frozen {
            _state: state,
            bytes,
        });
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let frozen = slot.borrow();
        let Some(frozen) = frozen.as_ref() else {
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
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn word(&mut self) -> Result<u64, i32> {
        let (head, tail) = self.0.split_at_checked(8).ok_or(EINVAL)?;
        self.0 = tail;
        Ok(u64::from_le_bytes(head.try_into().unwrap()))
    }
}
fn restore(input: &[u8]) -> Result<(), i32> {
    let mut reader = Reader(input);
    if reader.word()? != KEY {
        return Err(EINVAL);
    }
    let next_id = c_int::try_from(reader.word()?).map_err(|_| EINVAL)?;
    let segments = usize::try_from(reader.word()?).map_err(|_| EINVAL)?;
    let attachments = usize::try_from(reader.word()?).map_err(|_| EINVAL)?;
    let removed = usize::try_from(reader.word()?).map_err(|_| EINVAL)?;
    if next_id <= 0
        || segments > input.len() / 56
        || attachments > input.len() / 24
        || removed > input.len() / 8
    {
        return Err(EINVAL);
    }
    let mut restored = IpcState {
        segments: HashMap::new(),
        attachments: HashMap::new(),
        sets: HashMap::new(),
        removed: HashSet::new(),
        next_id,
    };
    let mut all = HashMap::new();
    for _ in 0..segments {
        let id = c_int::try_from(reader.word()?).map_err(|_| EINVAL)?;
        let namespace = reader.word()?;
        let key = reader.word()? as u32 as i32;
        let segsz = reader.word()? as usize;
        let fork_slot = reader.word()? as usize;
        let private_lock = reader.word()?;
        let active = reader.word()?;
        if id <= 0
            || segsz == 0
            || fork_slot < kinakaze_alloc::ARENA_BASE + kinakaze_alloc::HEADER_PAGE_SIZE
            || fork_slot > kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE - 8
            || !fork_slot.is_multiple_of(8)
            || active > 1
            || all.contains_key(&id)
        {
            return Err(EINVAL);
        }
        let mapping = unsafe { (*(fork_slot as *const AtomicUsize)).load(Ordering::Acquire) };
        let control = map_control(mapping as _)?;
        let segment = Arc::new(Segment {
            namespace,
            key,
            mapping,
            fork_slot,
            private_lock,
            control: control as usize,
            segsz,
        });
        if active != 0 {
            restored.segments.insert(id, segment.clone());
        }
        all.insert(id, segment);
    }
    for _ in 0..attachments {
        let address = reader.word()? as usize;
        let id = c_int::try_from(reader.word()?).map_err(|_| EINVAL)?;
        let read_only = reader.word()?;
        let segment = all.get(&id).ok_or(EINVAL)?.clone();
        if address == 0 || read_only > 1 || restored.attachments.contains_key(&address) {
            return Err(EINVAL);
        }
        {
            let _lock = shm_lock(&segment)?;
            let header = unsafe { &mut *(segment.control as *mut ShmHeader) };
            header.nattch += 1;
            header.lpid = own_pid();
            header.atime = now_seconds();
        }
        restored.attachments.insert(
            address,
            ShmAttachment {
                id,
                segment,
                read_only: read_only != 0,
            },
        );
    }
    for _ in 0..removed {
        restored
            .removed
            .insert(c_int::try_from(reader.word()?).map_err(|_| EINVAL)?);
    }
    if !reader.0.is_empty() {
        return Err(EINVAL);
    }
    *state().lock().map_err(|_| EIO)? = restored;
    Ok(())
}
unsafe extern "system" fn child(input: *const u8, size: usize) -> i32 {
    if input.is_null() || size > isize::MAX as usize {
        return EINVAL;
    }
    restore(unsafe { core::slice::from_raw_parts(input, size) })
        .err()
        .unwrap_or(0)
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 90,
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
