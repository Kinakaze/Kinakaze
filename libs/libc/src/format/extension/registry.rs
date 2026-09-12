//! Callback ownership and fork serialization, separate from printf parsing.
use super::{ArgInfo, Printer, VaReader};
use std::{
    cell::RefCell,
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Default)]
pub(super) struct Handler {
    pub printer: usize,
    pub arginfo: usize,
}
struct Registry {
    handlers: [Handler; 256],
    readers: [usize; 248],
    used: usize,
    modifiers: Vec<Vec<u8>>,
}
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    handlers: [Handler {
        printer: 0,
        arginfo: 0,
    }; 256],
    readers: [0; 248],
    used: 0,
    modifiers: Vec::new(),
});
static ACTIVE: AtomicBool = AtomicBool::new(false);
const MAGIC: u64 = u64::from_le_bytes(*b"CYPRTF01");
const FIXED: usize = 24 + 256 * 16 + 248 * 8;
thread_local! { static PREPARED: RefCell<Option<MutexGuard<'static, Registry>>> = const { RefCell::new(None) }; }

fn register() -> Result<(), i32> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    if *REGISTERED.get_or_init(|| {
        kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
            abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
            priority: 500,
            key: MAGIC,
            prepare: Some(prepare),
            snapshot: Some(snapshot),
            parent: Some(parent),
            child: Some(child),
        })
    }) {
        Ok(())
    } else {
        Err(kinakaze_vfs::EIO)
    }
}
fn failure(error: i32) -> i32 {
    crate::set_errno(error);
    -1
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_register_printf_specifier(
    spec: i32,
    printer: Option<Printer>,
    arginfo: Option<ArgInfo>,
) -> i32 {
    if !(0..256).contains(&spec) {
        return failure(kinakaze_vfs::EINVAL);
    }
    if let Err(e) = register() {
        return failure(e);
    }
    let Ok(mut registry) = REGISTRY.lock() else {
        return failure(kinakaze_vfs::EIO);
    };
    registry.handlers[spec as usize] = Handler {
        printer: printer.map_or(0, |f| f as usize),
        arginfo: arginfo.map_or(0, |f| f as usize),
    };
    ACTIVE.store(true, Ordering::Release);
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_register_printf_type(reader: Option<VaReader>) -> i32 {
    let Some(reader) = reader else {
        return failure(kinakaze_vfs::EINVAL);
    };
    if let Err(e) = register() {
        return failure(e);
    }
    let Ok(mut registry) = REGISTRY.lock() else {
        return failure(kinakaze_vfs::EIO);
    };
    let slot = registry.used;
    if slot == registry.readers.len() {
        return failure(kinakaze_vfs::ENOSPC);
    }
    registry.readers[slot] = reader as usize;
    registry.used += 1;
    ACTIVE.store(true, Ordering::Release);
    (slot + 8) as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_register_printf_modifier(text: *const i32) -> i32 {
    if text.is_null() {
        return failure(kinakaze_vfs::EINVAL);
    }
    let mut length = 0;
    loop {
        let c = unsafe { *text.add(length) };
        if c == 0 {
            break;
        }
        if !(1..=255).contains(&c) {
            return failure(kinakaze_vfs::EINVAL);
        }
        length += 1;
    }
    if length == 0 {
        return failure(kinakaze_vfs::EINVAL);
    }
    if let Err(e) = register() {
        return failure(e);
    }
    let Ok(mut registry) = REGISTRY.lock() else {
        return failure(kinakaze_vfs::EIO);
    };
    if registry.modifiers.len() == 16 {
        return failure(kinakaze_vfs::ENOSPC);
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(length).is_err() || registry.modifiers.try_reserve(1).is_err() {
        return failure(kinakaze_vfs::ENOMEM);
    }
    for i in 0..length {
        bytes.push(unsafe { *text.add(i) } as u8);
    }
    let bit = 1 << registry.modifiers.len();
    registry.modifiers.push(bytes);
    ACTIVE.store(true, Ordering::Release);
    bit
}

pub(super) fn handler(spec: u8) -> Result<Handler, i32> {
    if !ACTIVE.load(Ordering::Acquire) {
        return Ok(Handler::default());
    }
    REGISTRY
        .lock()
        .map(|r| r.handlers[spec as usize])
        .map_err(|_| kinakaze_vfs::EIO)
}
pub(super) fn reader(kind: i32) -> Result<VaReader, i32> {
    let r = REGISTRY.lock().map_err(|_| kinakaze_vfs::EIO)?;
    let index = usize::try_from(kind - 8).map_err(|_| kinakaze_vfs::EINVAL)?;
    let address = *r
        .readers
        .get(index)
        .filter(|&&p| p != 0)
        .ok_or(kinakaze_vfs::EINVAL)?;
    Ok(unsafe { core::mem::transmute::<usize, VaReader>(address) })
}
pub(crate) unsafe fn modifier(text: *const i8) -> (usize, u16) {
    if !ACTIVE.load(Ordering::Acquire) {
        return (0, 0);
    }
    let Ok(r) = REGISTRY.lock() else {
        return (0, 0);
    };
    let mut best = (0, 0);
    // Longest match, with the most recently registered bit winning ties.
    for (i, modifier) in r.modifiers.iter().enumerate().rev() {
        if modifier.len() > best.0
            && modifier
                .iter()
                .enumerate()
                .all(|(n, &c)| unsafe { *text.add(n) as u8 == c })
        {
            best = (modifier.len(), 1 << i);
        }
    }
    best
}

unsafe extern "system" fn prepare() -> i32 {
    PREPARED.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        match REGISTRY.lock() {
            Ok(r) => {
                *slot = Some(r);
                0
            }
            Err(_) => kinakaze_vfs::EIO,
        }
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    PREPARED.with(|slot| {
        let guard = slot.borrow();
        let Some(r) = guard.as_ref() else {
            return -(kinakaze_vfs::EINVAL as isize);
        };
        let size = FIXED + r.modifiers.iter().map(|s| 8 + s.len()).sum::<usize>();
        if output.is_null() {
            return size as isize;
        }
        if capacity < size {
            return -(kinakaze_vfs::EINVAL as isize);
        }
        let out = unsafe { core::slice::from_raw_parts_mut(output, size) };
        let mut offset = 0;
        let mut word = |value: u64| {
            out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            offset += 8;
        };
        word(MAGIC);
        word(r.used as u64);
        word(r.modifiers.len() as u64);
        for h in r.handlers {
            word(h.printer as u64);
            word(h.arginfo as u64);
        }
        for reader in r.readers {
            word(reader as u64);
        }
        for modifier in &r.modifiers {
            out[offset..offset + 8].copy_from_slice(&(modifier.len() as u64).to_le_bytes());
            offset += 8;
            out[offset..offset + modifier.len()].copy_from_slice(modifier);
            offset += modifier.len();
        }
        size as isize
    })
}
unsafe extern "system" fn parent(_: i32) {
    PREPARED.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length < FIXED {
        return kinakaze_vfs::EINVAL;
    }
    let data = unsafe { core::slice::from_raw_parts(input, length) };
    let word = |n: usize| u64::from_le_bytes(data[n..n + 8].try_into().unwrap());
    let used = word(8) as usize;
    let count = word(16) as usize;
    if word(0) != MAGIC || used > 248 || count > 16 {
        return kinakaze_vfs::EINVAL;
    }
    let mut restored = Registry {
        handlers: [Handler::default(); 256],
        readers: [0; 248],
        used,
        modifiers: Vec::new(),
    };
    let mut offset = 24;
    for h in &mut restored.handlers {
        *h = Handler {
            printer: word(offset) as usize,
            arginfo: word(offset + 8) as usize,
        };
        offset += 16;
    }
    for r in &mut restored.readers {
        *r = word(offset) as usize;
        offset += 8;
    }
    for _ in 0..count {
        if length - offset < 8 {
            return kinakaze_vfs::EINVAL;
        }
        let n = word(offset) as usize;
        offset += 8;
        if n == 0 || n > length - offset || data[offset..offset + n].contains(&0) {
            return kinakaze_vfs::EINVAL;
        }
        restored.modifiers.push(data[offset..offset + n].to_vec());
        offset += n;
    }
    if offset != length {
        return kinakaze_vfs::EINVAL;
    }
    let Ok(mut r) = REGISTRY.lock() else {
        return kinakaze_vfs::EIO;
    };
    *r = restored;
    ACTIVE.store(true, Ordering::Release);
    0
}
