//! Reentrant search.h hash tables. Entries retain their address and borrow the
//! caller's key/data; a single guest allocation survives fork without fixups.
use core::ffi::{CStr, c_char, c_int, c_void};
use core::{mem, ptr};
use kinakaze_alloc::guest;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Entry {
    key: *mut c_char,
    data: *mut c_void,
}
#[repr(C)]
struct Bucket {
    hash: u64,
    entry: Entry,
}
#[repr(C)]
pub struct Table {
    buckets: *mut Bucket,
    size: u32,
    filled: u32,
}
fn failure(error: i32) -> i32 {
    crate::set_errno(error);
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_hcreate_r(count: usize, table: *mut Table) -> c_int {
    if table.is_null() {
        return failure(kinakaze_vfs::EINVAL);
    }
    if unsafe { !(*table).buckets.is_null() } {
        return 0;
    }
    let Some(size) = count
        .max(8)
        .checked_next_power_of_two()
        .filter(|n| *n <= u32::MAX as usize)
    else {
        return failure(kinakaze_vfs::ENOMEM);
    };
    let Some(bytes) = size.checked_mul(mem::size_of::<Bucket>()) else {
        return failure(kinakaze_vfs::ENOMEM);
    };
    let buckets = unsafe { guest::malloc(bytes) }.cast::<Bucket>();
    if buckets.is_null() {
        return failure(kinakaze_vfs::ENOMEM);
    }
    unsafe {
        ptr::write_bytes(buckets.cast::<u8>(), 0, bytes);
        table.write(Table {
            buckets,
            size: size as u32,
            filled: 0,
        });
    }
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_hsearch_r(
    item: Entry,
    action: c_int,
    result: *mut *mut Entry,
    table: *mut Table,
) -> c_int {
    if !result.is_null() {
        unsafe {
            *result = ptr::null_mut();
        }
    }
    if table.is_null() || result.is_null() || item.key.is_null() || !(0..=1).contains(&action) {
        return failure(kinakaze_vfs::EINVAL);
    }
    let t = unsafe { &mut *table };
    if t.buckets.is_null() || !t.size.is_power_of_two() {
        return failure(kinakaze_vfs::EINVAL);
    }
    let key = unsafe { CStr::from_ptr(item.key) }.to_bytes();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in key {
        hash = (hash ^ *byte as u64).wrapping_mul(0x100000001b3);
    }
    hash = hash.max(1);
    let mask = t.size as usize - 1;
    let mut index = hash as usize & mask;
    // An odd step visits every slot of a power-of-two table, reducing primary
    // clustering while keeping every returned ENTRY pointer stable.
    let step = ((hash >> 32) as usize | 1) & mask;
    for _ in 0..t.size {
        let bucket = unsafe { &mut *t.buckets.add(index) };
        if bucket.hash == 0 {
            if action == 0 {
                break;
            }
            bucket.hash = hash;
            bucket.entry = item;
            t.filled += 1;
            unsafe {
                *result = &raw mut bucket.entry;
            }
            return 1;
        }
        if bucket.hash == hash && unsafe { CStr::from_ptr(bucket.entry.key) }.to_bytes() == key {
            unsafe {
                *result = &raw mut bucket.entry;
            }
            return 1;
        }
        index = (index + step) & mask;
    }
    failure(if action == 1 {
        kinakaze_vfs::ENOMEM
    } else {
        kinakaze_vfs::ESRCH
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_hdestroy_r(table: *mut Table) {
    if table.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return;
    }
    unsafe {
        guest::free((*table).buckets.cast());
        table.write(Table {
            buckets: ptr::null_mut(),
            size: 0,
            filled: 0,
        });
    }
}
