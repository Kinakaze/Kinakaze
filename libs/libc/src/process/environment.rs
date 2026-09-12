//! Guest environment storage and its explicit fork state.
//!
//! Published arrays and strings belong to runtime guest memory. A fresh DLL
//! restores only their addresses and the COPY binding, never a Rust allocation.
use core::sync::atomic::{AtomicPtr, Ordering};
use core::{
    cell::UnsafeCell,
    ffi::{CStr, c_char, c_int},
    ptr,
};
use std::{
    cell::RefCell,
    sync::{Mutex, MutexGuard},
};

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static kinakaze_abi_environ: EnvironPointer = EnvironPointer::new();

#[repr(C)]
pub struct EnvironPointer {
    block: UnsafeCell<*mut *mut c_char>,
    location: AtomicPtr<*mut *mut c_char>,
}

// Guest writes and environment mutations require caller synchronization, as
// with the Linux ABI. The binding itself is atomically published by the loader.
unsafe impl Sync for EnvironPointer {}

impl EnvironPointer {
    const fn new() -> Self {
        Self {
            block: UnsafeCell::new(ptr::null_mut()),
            location: AtomicPtr::new(ptr::null_mut()),
        }
    }
    pub fn get(&self) -> *mut *mut c_char {
        let target = self.location.load(Ordering::Acquire);
        unsafe {
            if target.is_null() {
                self.block.get().read()
            } else {
                target.read()
            }
        }
    }
    fn set(&self, block: *mut *mut c_char) {
        unsafe {
            self.block.get().write(block);
            let target = self.location.load(Ordering::Acquire);
            if !target.is_null() {
                target.write(block);
            }
        }
    }
    /// The loader supplies writable guest storage before publishing startup envp.
    pub(crate) unsafe fn redirect(&self, target: *mut *mut *mut c_char) {
        let current = self.get();
        self.location.store(target, Ordering::Release);
        self.set(current);
    }
}

#[derive(Clone, Copy)]
struct OwnedBlock {
    address: usize,
    capacity: usize,
}
static OWNED: Mutex<OwnedBlock> = Mutex::new(OwnedBlock {
    address: 0,
    capacity: 0,
});

fn block_length(block: *mut *mut c_char) -> usize {
    if block.is_null() {
        return 0;
    }
    let mut length = 0;
    unsafe {
        while !block.add(length).read().is_null() {
            length += 1;
        }
    }
    length
}

impl OwnedBlock {
    /// Retain borrowed strings, as putenv/direct environ assignment require.
    /// Only the vector is ours; its allocation is managed across fork.
    fn reserve(
        &mut self,
        current: *mut *mut c_char,
        length: usize,
        required: usize,
    ) -> Result<*mut *mut c_char, i32> {
        let mut block = self.address as *mut *mut c_char;
        if required > self.capacity {
            let capacity = required.checked_next_power_of_two().ok_or(crate::ENOMEM)?;
            let bytes = capacity
                .checked_mul(size_of::<*mut c_char>())
                .ok_or(crate::ENOMEM)?;
            let next = unsafe { kinakaze_alloc::guest::malloc(bytes) }.cast::<*mut c_char>();
            if next.is_null() {
                return Err(crate::ENOMEM);
            }
            unsafe {
                if length != 0 {
                    ptr::copy_nonoverlapping(current, next, length);
                }
                next.add(length).write(ptr::null_mut());
                kinakaze_alloc::guest::free(block.cast());
            }
            self.address = next as usize;
            self.capacity = capacity;
            block = next;
        } else if current != block {
            unsafe {
                if length != 0 {
                    ptr::copy(current, block, length);
                }
                block.add(length).write(ptr::null_mut());
            }
        }
        Ok(block)
    }
}

pub(crate) fn adopt(block: *mut *mut c_char) {
    let _guard = OWNED.lock().unwrap_or_else(|error| error.into_inner());
    kinakaze_abi_environ.set(block);
}

fn failure(error: i32) -> c_int {
    crate::set_errno(error);
    -1
}

/// # Safety
/// Both arguments are readable NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setenv(
    name: *const c_char,
    value: *const c_char,
    overwrite: c_int,
) -> c_int {
    if name.is_null() || value.is_null() {
        return failure(crate::EINVAL);
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    let value = unsafe { CStr::from_ptr(value) }.to_bytes();
    if name.is_empty() || name.contains(&b'=') {
        return failure(crate::EINVAL);
    }
    let mut owned = OWNED.lock().unwrap_or_else(|error| error.into_inner());
    let current = kinakaze_abi_environ.get();
    let length = block_length(current);
    let existing = (0..length).find(|&index| {
        unsafe { split(current.add(index).read()) }.is_some_and(|(key, _)| key == name)
    });
    if existing.is_some() && overwrite == 0 {
        return 0;
    }
    let Some(bytes) = name
        .len()
        .checked_add(value.len())
        .and_then(|n| n.checked_add(2))
    else {
        return failure(crate::ENOMEM);
    };
    let replacement = unsafe { kinakaze_alloc::guest::malloc(bytes) };
    if replacement.is_null() {
        return failure(crate::ENOMEM);
    }
    unsafe {
        ptr::copy_nonoverlapping(name.as_ptr(), replacement, name.len());
        replacement.add(name.len()).write(b'=');
        ptr::copy_nonoverlapping(value.as_ptr(), replacement.add(name.len() + 1), value.len());
        replacement.add(bytes - 1).write(0);
    }
    let required = length + if existing.is_some() { 1 } else { 2 };
    let block = match owned.reserve(current, length, required) {
        Ok(block) => block,
        Err(error) => {
            unsafe { kinakaze_alloc::guest::free(replacement) };
            return failure(error);
        }
    };
    unsafe {
        block
            .add(existing.unwrap_or(length))
            .write(replacement.cast());
        if existing.is_none() {
            block.add(length + 1).write(ptr::null_mut());
        }
    }
    // Old strings remain valid for prior getenv readers. No guest variable is
    // mirrored into Windows' private loader/control environment.
    kinakaze_abi_environ.set(block);
    0
}

/// # Safety
/// `name` is a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_unsetenv(name: *const c_char) -> c_int {
    if name.is_null() {
        return failure(crate::EINVAL);
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    if name.is_empty() || name.contains(&b'=') {
        return failure(crate::EINVAL);
    }
    let mut owned = OWNED.lock().unwrap_or_else(|error| error.into_inner());
    let current = kinakaze_abi_environ.get();
    let length = block_length(current);
    if length == 0 {
        return 0;
    }
    let block = match owned.reserve(current, length, length + 1) {
        Ok(block) => block,
        Err(error) => return failure(error),
    };
    let mut kept = 0;
    for index in 0..length {
        let entry = unsafe { block.add(index).read() };
        if unsafe { split(entry) }.is_none_or(|(key, _)| key != name) {
            unsafe {
                block.add(kept).write(entry);
            }
            kept += 1;
        }
    }
    unsafe {
        block.add(kept).write(ptr::null_mut());
    }
    kinakaze_abi_environ.set(block);
    0
}

/// # Safety
/// `string` stays writable and live while installed in the environment.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_putenv(string: *mut c_char) -> c_int {
    if string.is_null() {
        return failure(crate::EINVAL);
    }
    let text = unsafe { CStr::from_ptr(string) }.to_bytes();
    let Some(equals) = text.iter().position(|&byte| byte == b'=') else {
        return unsafe { kinakaze_abi_unsetenv(string) };
    };
    if equals == 0 {
        return failure(crate::EINVAL);
    }
    let name = &text[..equals];
    let mut owned = OWNED.lock().unwrap_or_else(|error| error.into_inner());
    let current = kinakaze_abi_environ.get();
    let length = block_length(current);
    let existing = (0..length).find(|&index| {
        unsafe { split(current.add(index).read()) }.is_some_and(|(key, _)| key == name)
    });
    let required = length + if existing.is_some() { 1 } else { 2 };
    let block = match owned.reserve(current, length, required) {
        Ok(block) => block,
        Err(error) => return failure(error),
    };
    unsafe {
        block.add(existing.unwrap_or(length)).write(string);
        if existing.is_none() {
            block.add(length + 1).write(ptr::null_mut());
        }
    }
    kinakaze_abi_environ.set(block);
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_clearenv() -> c_int {
    let _guard = OWNED.lock().unwrap_or_else(|error| error.into_inner());
    kinakaze_abi_environ.set(ptr::null_mut());
    0
}

const FORK_MAGIC: u64 = 0x4352_5945_4e56_4631;
const FORK_BYTES: usize = 40;
thread_local! {
    static FORK_GUARD: RefCell<Option<MutexGuard<'static, OwnedBlock>>> = const { RefCell::new(None) };
}
unsafe extern "system" fn fork_prepare() -> i32 {
    FORK_GUARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        *slot = Some(OWNED.lock().unwrap_or_else(|error| error.into_inner()));
        0
    })
}
unsafe extern "system" fn fork_snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return FORK_BYTES as isize;
    }
    if capacity < FORK_BYTES {
        return -(crate::EINVAL as isize);
    }
    FORK_GUARD.with(|slot| {
        let slot = slot.borrow();
        let Some(owned) = slot.as_ref() else {
            return -(crate::EINVAL as isize);
        };
        let values = [
            FORK_MAGIC,
            kinakaze_abi_environ.get() as u64,
            kinakaze_abi_environ.location.load(Ordering::Acquire) as u64,
            owned.address as u64,
            owned.capacity as u64,
        ];
        for (index, value) in values.into_iter().enumerate() {
            unsafe {
                ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), output.add(index * 8), 8);
            }
        }
        FORK_BYTES as isize
    })
}
unsafe extern "system" fn fork_parent(_result: i32) {
    FORK_GUARD.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn fork_child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != FORK_BYTES {
        return crate::EINVAL;
    }
    let bytes = unsafe { core::slice::from_raw_parts(input, length) };
    let word =
        |index: usize| u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
    if word(0) != FORK_MAGIC || (word(3) == 0) != (word(4) == 0) {
        return crate::EINVAL;
    }
    let mut owned = OWNED.lock().unwrap_or_else(|error| error.into_inner());
    *owned = OwnedBlock {
        address: word(3) as usize,
        capacity: word(4) as usize,
    };
    kinakaze_abi_environ
        .location
        .store(word(2) as _, Ordering::Release);
    kinakaze_abi_environ.set(word(1) as _);
    0
}
extern "C" fn register_fork_state() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 500,
        key: FORK_MAGIC,
        prepare: Some(fork_prepare),
        snapshot: Some(fork_snapshot),
        parent: Some(fork_parent),
        child: Some(fork_child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static FORK_INITIALIZER: extern "C" fn() = register_fork_state;

/// Splits an `environ` entry into its name and the address of its value.
///
/// # Safety
///
/// `entry` must be a null-terminated string.
unsafe fn split(entry: *mut c_char) -> Option<(&'static [u8], *mut c_char)> {
    // SAFETY: forwarded from this function's contract.
    let bytes = unsafe { CStr::from_ptr(entry) }.to_bytes();
    let equals = bytes.iter().position(|byte| *byte == b'=')?;
    // SAFETY: the '=' is inside the string, so one past it is still in bounds:
    // at worst it addresses the terminator.
    let value = unsafe { entry.add(equals + 1) };
    Some((&bytes[..equals], value))
}

/// `getenv`, scanning the `environ` block.
///
/// The returned pointer addresses the value inside the entry itself, as glibc's
/// does. It stays valid until that entry is replaced or removed.
///
/// # Safety
///
/// `name` must be null-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getenv(name: *const c_char) -> *mut c_char {
    if name.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let wanted = unsafe { CStr::from_ptr(name) }.to_bytes();
    if wanted.is_empty() || wanted.contains(&b'=') {
        return ptr::null_mut();
    }
    let trace = std::env::var("KINAKAZE_ENV_TRACE")
        .ok()
        .is_some_and(|filter| filter.is_empty() || wanted.starts_with(filter.as_bytes()));
    let block = kinakaze_abi_environ.get();
    if block.is_null() {
        if trace {
            eprintln!(
                "kinakaze: getenv({}) -> null (environment is not published)",
                String::from_utf8_lossy(wanted)
            );
        }
        return ptr::null_mut();
    }
    let mut cursor = 0;
    loop {
        // SAFETY: `environ` is a null-terminated array of C strings.
        let entry = unsafe { *block.add(cursor) };
        if entry.is_null() {
            if trace {
                eprintln!(
                    "kinakaze: getenv({}) -> null",
                    String::from_utf8_lossy(wanted)
                );
            }
            return ptr::null_mut();
        }
        // SAFETY: entries in `environ` are null-terminated strings.
        if let Some((key, value)) = unsafe { split(entry) }
            && key == wanted
        {
            if trace {
                // SAFETY: `value` points inside this terminated environment entry.
                let shown = unsafe { CStr::from_ptr(value) }.to_bytes();
                eprintln!(
                    "kinakaze: getenv({}) -> {}",
                    String::from_utf8_lossy(wanted),
                    String::from_utf8_lossy(shown)
                );
            }
            return value;
        }
        cursor += 1;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_secure_getenv(name: *const c_char) -> *mut c_char {
    unsafe { kinakaze_abi_getenv(name) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___secure_getenv(name: *const c_char) -> *mut c_char {
    unsafe { kinakaze_abi_getenv(name) }
}
