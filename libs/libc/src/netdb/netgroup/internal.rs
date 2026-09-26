//! Reentrant private netgroup state, owned by the caller and backed by guest
//! allocations. Separate enumerators and fork children never share Rust state.
use super::*;
use core::{ffi::c_void, ptr};
use kinakaze_alloc::guest;
#[repr(C)]
pub struct Netgrent {
    kind: i32,
    values: [*mut c_char; 3],
    data: *mut u8,
    size: usize,
    position: usize,
    first: i32,
    known: *mut c_void,
    needed: *mut c_void,
    action: *mut c_void,
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___internal_endnetgrent(state: *mut Netgrent) {
    if state.is_null() {
        return;
    }
    unsafe {
        guest::free((*state).data);
        ptr::write_bytes(state, 0, 1);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___internal_setnetgrent(
    name: *const c_char,
    state: *mut Netgrent,
) -> c_int {
    if name.is_null() || state.is_null() {
        crate::set_errno(22);
        return 0;
    }
    unsafe {
        kinakaze_abi___internal_endnetgrent(state);
    }
    let mut entries = Vec::new();
    let found = super::enumeration::expand(
        unsafe { CStr::from_ptr(name) }.to_bytes(),
        &mut HashSet::new(),
        &mut entries,
    );
    match found {
        Err(error) => {
            crate::set_errno(error);
            return 0;
        }
        Ok(false) => return 0,
        Ok(true) => {}
    }
    let mut bytes = Vec::new();
    for triple in entries {
        for value in triple {
            if let Some(value) = value {
                bytes.extend_from_slice(&(value.as_bytes_with_nul().len() as u32).to_le_bytes());
                bytes.extend_from_slice(value.as_bytes_with_nul());
            } else {
                bytes.extend_from_slice(&0u32.to_le_bytes());
            }
        }
    }
    let data = unsafe { guest::malloc(bytes.len().max(1)) };
    if data.is_null() {
        crate::set_errno(12);
        return 0;
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        (*state).data = data;
        (*state).size = bytes.len();
        (*state).first = 1;
    }
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___internal_getnetgrent_r(
    host: *mut *mut c_char,
    user: *mut *mut c_char,
    domain: *mut *mut c_char,
    state: *mut Netgrent,
    buffer: *mut c_char,
    length: usize,
    error: *mut c_int,
) -> c_int {
    let result = (|| {
        if host.is_null()
            || user.is_null()
            || domain.is_null()
            || state.is_null()
            || buffer.is_null()
        {
            return Err(22);
        }
        let state = unsafe { &mut *state };
        if state.data.is_null() || state.position == state.size {
            return Ok(0);
        }
        let bytes = unsafe { core::slice::from_raw_parts(state.data, state.size) };
        let mut at = state.position;
        let mut fields = [&[][..]; 3];
        let mut required = 0usize;
        for field in &mut fields {
            let header = bytes.get(at..at.checked_add(4).ok_or(5)?).ok_or(5)?;
            let size = u32::from_le_bytes(header.try_into().unwrap()) as usize;
            at += 4;
            let end = at.checked_add(size).ok_or(5)?;
            *field = bytes.get(at..end).ok_or(5)?;
            if size != 0 && field.last() != Some(&0) {
                return Err(5);
            }
            required = required.checked_add(size).ok_or(34)?;
            at = end;
        }
        if required > length {
            return Err(34);
        }
        let mut output = buffer;
        for (index, (slot, value)) in [host, user, domain].into_iter().zip(fields).enumerate() {
            unsafe {
                let address = if value.is_empty() {
                    ptr::null_mut()
                } else {
                    ptr::copy_nonoverlapping(value.as_ptr().cast(), output, value.len());
                    let address = output;
                    output = output.add(value.len());
                    address
                };
                slot.write(address);
                state.values[index] = address;
            }
        }
        state.position = at;
        state.first = 0;
        state.kind = 0;
        Ok(1)
    })();
    result.unwrap_or_else(|code| {
        if !error.is_null() {
            unsafe {
                error.write(code);
            }
        }
        crate::set_errno(code);
        0
    })
}
