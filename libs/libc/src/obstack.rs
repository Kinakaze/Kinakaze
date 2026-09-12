//! GNU object-stack ABI, with guest callbacks and one owner for each chunk chain.
//! Layout follows the Linux x86-64 public obstack.h, not Windows C long.
use crate::copied::{CopiedInt, CopiedPointer};
use core::ffi::{c_char, c_int, c_void};
use core::{mem, ptr};

const EXTRA_ARG: u32 = 1;
const MAYBE_EMPTY: u32 = 2;
const CONTENTS_OFFSET: usize = 16;

struct FormatSink(*mut Obstack, usize);
impl crate::format::Sink for FormatSink {
    fn written(&self) -> usize {
        self.1
    }
    fn write(&mut self, bytes: &[u8]) {
        let length = c_int::try_from(bytes.len()).unwrap_or_else(|_| allocation_failed());
        unsafe {
            let stack = &*self.0;
            if (stack.chunk_limit as usize).saturating_sub(stack.next_free as usize) < bytes.len() {
                kinakaze_abi__obstack_newchunk(self.0, length);
            }
            let stack = &mut *self.0;
            ptr::copy_nonoverlapping(bytes.as_ptr(), stack.next_free, bytes.len());
            stack.next_free = stack.next_free.add(bytes.len());
        }
        self.1 = self
            .1
            .checked_add(bytes.len())
            .unwrap_or_else(|| allocation_failed());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_obstack_vprintf(
    stack: *mut Obstack,
    format: *const c_char,
    args: *mut crate::format::VaList,
) -> c_int {
    if stack.is_null() || format.is_null() || args.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let mut sink = FormatSink(stack, 0);
    unsafe {
        crate::format::format(&mut sink, format, &mut *args);
    }
    match c_int::try_from(sink.1) {
        Ok(count) => count,
        Err(_) => {
            crate::set_errno(kinakaze_vfs::EOVERFLOW);
            -1
        }
    }
}

#[repr(C)]
pub struct ObstackChunk {
    pub limit: *mut u8,
    pub prev: *mut ObstackChunk,
    pub contents: [u8; 4],
}

#[repr(C)]
pub struct Obstack {
    pub chunk_size: i64,
    pub chunk: *mut ObstackChunk,
    pub object_base: *mut u8,
    pub next_free: *mut u8,
    pub chunk_limit: *mut u8,
    pub temp: usize,
    pub alignment_mask: c_int,
    pub chunkfun: *mut c_void,
    pub freefun: *mut c_void,
    pub extra_arg: *mut c_void,
    pub flags: u32,
}

impl Obstack {
    fn empty() -> Self {
        Self {
            chunk_size: 0,
            chunk: ptr::null_mut(),
            object_base: ptr::null_mut(),
            next_free: ptr::null_mut(),
            chunk_limit: ptr::null_mut(),
            temp: 0,
            alignment_mask: 0,
            chunkfun: ptr::null_mut(),
            freefun: ptr::null_mut(),
            extra_arg: ptr::null_mut(),
            flags: 0,
        }
    }
}

type Allocate = unsafe extern "sysv64" fn(i64) -> *mut c_void;
type AllocateWithArg = unsafe extern "sysv64" fn(*mut c_void, i64) -> *mut c_void;
type Release = unsafe extern "sysv64" fn(*mut c_void);
type ReleaseWithArg = unsafe extern "sysv64" fn(*mut c_void, *mut c_void);

#[unsafe(no_mangle)]
pub static kinakaze_abi_obstack_alloc_failed_handler: CopiedPointer =
    CopiedPointer::with_value(default_failure as *const () as *mut c_char);
#[unsafe(no_mangle)]
pub static kinakaze_abi_obstack_exit_failure: CopiedInt = CopiedInt::new(1);

extern "sysv64" fn default_failure() {
    eprintln!("obstack: memory allocation failed");
    crate::process::kinakaze_abi_exit(kinakaze_abi_obstack_exit_failure.get());
}

fn allocation_failed() -> ! {
    let address = kinakaze_abi_obstack_alloc_failed_handler.get();
    if !address.is_null() {
        // SAFETY: the public variable's ABI requires a SysV void(void) callback.
        let handler: unsafe extern "sysv64" fn() = unsafe { mem::transmute(address) };
        unsafe { handler() };
    }
    // The documented handler must not return. Do not continue with null storage.
    std::process::abort()
}

unsafe fn allocate(stack: &Obstack, bytes: usize) -> *mut ObstackChunk {
    if bytes > i64::MAX as usize {
        allocation_failed();
    }
    let address = if stack.chunkfun.is_null() {
        unsafe { kinakaze_alloc::c::malloc(bytes).cast() }
    } else if stack.flags & EXTRA_ARG != 0 {
        let callback: AllocateWithArg = unsafe { mem::transmute(stack.chunkfun) };
        unsafe { callback(stack.extra_arg, bytes as i64) }
    } else {
        let callback: Allocate = unsafe { mem::transmute(stack.chunkfun) };
        unsafe { callback(bytes as i64) }
    };
    if address.is_null() {
        allocation_failed();
    }
    address.cast()
}

unsafe fn release(stack: &Obstack, chunk: *mut ObstackChunk) {
    if stack.freefun.is_null() {
        unsafe { kinakaze_alloc::c::free(chunk.cast()) };
    } else if stack.flags & EXTRA_ARG != 0 {
        let callback: ReleaseWithArg = unsafe { mem::transmute(stack.freefun) };
        unsafe { callback(stack.extra_arg, chunk.cast()) };
    } else {
        let callback: Release = unsafe { mem::transmute(stack.freefun) };
        unsafe { callback(chunk.cast()) };
    }
}

fn object_start(chunk: *mut ObstackChunk, mask: c_int) -> *mut u8 {
    let mask = mask as usize;
    ((chunk as usize + CONTENTS_OFFSET + mask) & !mask) as *mut u8
}

unsafe fn begin(stack: &mut Obstack, size: c_int, alignment: c_int) -> c_int {
    let alignment = if alignment == 0 { 16 } else { alignment };
    if size < 0 || alignment < 1 || !(alignment as u32).is_power_of_two() {
        allocation_failed();
    }
    stack.alignment_mask = alignment - 1;
    // Even a small requested chunk must hold its header and aligned object base.
    let minimum = CONTENTS_OFFSET + alignment as usize;
    let size = if size == 0 { 4064 } else { size as usize }.max(minimum);
    stack.chunk_size = size as i64;
    let chunk = unsafe { allocate(stack, size) };
    unsafe {
        (*chunk).prev = ptr::null_mut();
        (*chunk).limit = chunk.cast::<u8>().add(size);
        stack.chunk_limit = (*chunk).limit;
    }
    stack.chunk = chunk;
    stack.object_base = object_start(chunk, stack.alignment_mask);
    stack.next_free = stack.object_base;
    stack.flags &= EXTRA_ARG;
    1
}

/// The caller supplies a writable Linux obstack and callbacks with the declared ABI.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__obstack_begin(
    stack: *mut Obstack,
    size: c_int,
    alignment: c_int,
    alloc: Option<Allocate>,
    free: Option<Release>,
) -> c_int {
    if stack.is_null() {
        return 0;
    }
    unsafe { ptr::write(stack, Obstack::empty()) };
    let stack = unsafe { &mut *stack };
    stack.chunkfun = alloc.map_or(ptr::null_mut(), |function| {
        function as *const () as *mut c_void
    });
    stack.freefun = free.map_or(ptr::null_mut(), |function| {
        function as *const () as *mut c_void
    });
    stack.extra_arg = ptr::null_mut();
    stack.flags = 0;
    unsafe { begin(stack, size, alignment) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__obstack_begin_1(
    stack: *mut Obstack,
    size: c_int,
    alignment: c_int,
    alloc: Option<AllocateWithArg>,
    free: Option<ReleaseWithArg>,
    argument: *mut c_void,
) -> c_int {
    if stack.is_null() {
        return 0;
    }
    unsafe { ptr::write(stack, Obstack::empty()) };
    let stack = unsafe { &mut *stack };
    stack.chunkfun = alloc.map_or(ptr::null_mut(), |function| {
        function as *const () as *mut c_void
    });
    stack.freefun = free.map_or(ptr::null_mut(), |function| {
        function as *const () as *mut c_void
    });
    // Configure the callback mode before the very first allocation.
    stack.extra_arg = argument;
    stack.flags = EXTRA_ARG;
    unsafe { begin(stack, size, alignment) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__obstack_newchunk(stack: *mut Obstack, length: c_int) {
    let stack = unsafe { &mut *stack };
    if length < 0 {
        allocation_failed();
    }
    let object_size = (stack.next_free as usize)
        .checked_sub(stack.object_base as usize)
        .unwrap_or_else(|| allocation_failed());
    let required = object_size
        .checked_add(length as usize)
        .and_then(|size| size.checked_add(stack.alignment_mask as usize))
        .and_then(|size| size.checked_add(CONTENTS_OFFSET))
        .unwrap_or_else(|| allocation_failed());
    let size = required
        .checked_add(object_size / 8)
        .and_then(|size| size.checked_add(128))
        .unwrap_or(required)
        .max(stack.chunk_size as usize);
    let previous = stack.chunk;
    let chunk = unsafe { allocate(stack, size) };
    let base = object_start(chunk, stack.alignment_mask);
    unsafe {
        (*chunk).limit = chunk.cast::<u8>().add(size);
        (*chunk).prev = previous;
        ptr::copy_nonoverlapping(stack.object_base, base, object_size);
        // A finished empty object still owns an address in the old chunk.
        if stack.flags & MAYBE_EMPTY == 0
            && stack.object_base == object_start(previous, stack.alignment_mask)
        {
            (*chunk).prev = (*previous).prev;
            release(stack, previous);
        }
        stack.chunk_limit = (*chunk).limit;
        stack.next_free = base.add(object_size);
    }
    stack.chunk = chunk;
    stack.object_base = base;
    stack.flags &= !MAYBE_EMPTY;
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_obstack_free(stack: *mut Obstack, object: *mut c_void) {
    let stack = unsafe { &mut *stack };
    let mut chunk = stack.chunk;
    while !chunk.is_null() {
        if !object.is_null()
            && (chunk as usize) < object as usize
            && object as usize <= unsafe { (*chunk).limit } as usize
        {
            stack.chunk = chunk;
            stack.chunk_limit = unsafe { (*chunk).limit };
            stack.object_base = object.cast();
            stack.next_free = object.cast();
            return;
        }
        let previous = unsafe { (*chunk).prev };
        unsafe { release(stack, chunk) };
        chunk = previous;
        stack.flags |= MAYBE_EMPTY;
    }
    stack.chunk = ptr::null_mut();
    stack.object_base = ptr::null_mut();
    stack.next_free = ptr::null_mut();
    stack.chunk_limit = ptr::null_mut();
    if !object.is_null() {
        std::process::abort();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__obstack_memory_used(stack: *const Obstack) -> c_int {
    let mut chunk = unsafe { (*stack).chunk };
    let mut total = 0usize;
    while !chunk.is_null() {
        total = total.wrapping_add(unsafe { (*chunk).limit } as usize - chunk as usize);
        chunk = unsafe { (*chunk).prev };
    }
    total as c_int
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{Layout, alloc, dealloc};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Tracker {
        live: BTreeMap<usize, Layout>,
        allocations: usize,
    }
    unsafe extern "sysv64" fn tracked_alloc(argument: *mut c_void, size: i64) -> *mut c_void {
        let tracker = unsafe { &mut *argument.cast::<Tracker>() };
        let layout = Layout::from_size_align(size as usize, 16).unwrap();
        let address = unsafe { alloc(layout) };
        tracker.live.insert(address as usize, layout);
        tracker.allocations += 1;
        address.cast()
    }
    unsafe extern "sysv64" fn tracked_free(argument: *mut c_void, address: *mut c_void) {
        let tracker = unsafe { &mut *argument.cast::<Tracker>() };
        let layout = tracker
            .live
            .remove(&(address as usize))
            .expect("each chunk freed once");
        unsafe { dealloc(address.cast(), layout) };
    }

    #[test]
    fn linux_layout_and_callback_mode_survive_growth_rewind_and_full_free() {
        assert_eq!(mem::size_of::<Obstack>(), 88);
        assert_eq!(mem::offset_of!(Obstack, alignment_mask), 48);
        assert_eq!(mem::offset_of!(Obstack, chunkfun), 56);
        assert_eq!(mem::offset_of!(Obstack, flags), 80);
        let mut tracker = Tracker::default();
        let mut stack = mem::MaybeUninit::<Obstack>::uninit();
        unsafe {
            assert_eq!(
                kinakaze_abi__obstack_begin_1(
                    stack.as_mut_ptr(),
                    64,
                    32,
                    Some(tracked_alloc),
                    Some(tracked_free),
                    (&mut tracker as *mut Tracker).cast()
                ),
                1
            );
            let mut stack = stack.assume_init();
            assert_eq!(tracker.allocations, 1);
            assert_eq!(stack.object_base as usize % 32, 0);
            let first = stack.object_base;
            ptr::copy_nonoverlapping(b"first".as_ptr(), first, 5);
            // Finish the first object as the public header macros do.
            stack.object_base = first.add(32);
            stack.next_free = stack.object_base;
            kinakaze_abi__obstack_newchunk(&mut stack, 200);
            assert_eq!(tracker.live.len(), 2);
            ptr::copy_nonoverlapping(b"growing".as_ptr(), stack.next_free, 7);
            stack.next_free = stack.next_free.add(7);
            kinakaze_abi__obstack_newchunk(&mut stack, 900);
            assert_eq!(
                core::slice::from_raw_parts(stack.object_base, 7),
                b"growing"
            );
            assert_eq!(
                tracker.live.len(),
                2,
                "only uncommitted old object storage may be recycled"
            );
            assert!(kinakaze_abi__obstack_memory_used(&stack) > 900);
            kinakaze_abi_obstack_free(&mut stack, first.cast());
            assert_eq!(tracker.live.len(), 1);
            assert_eq!(stack.next_free, first);
            stack.flags |= MAYBE_EMPTY;
            kinakaze_abi__obstack_newchunk(&mut stack, 1000);
            assert_eq!(
                tracker.live.len(),
                2,
                "empty finished objects retain their chunk"
            );
            kinakaze_abi_obstack_free(&mut stack, ptr::null_mut());
            assert!(tracker.live.is_empty());
            assert!(stack.chunk.is_null());
        }
    }
}
